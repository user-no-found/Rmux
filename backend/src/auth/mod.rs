use crate::config::AppConfig;
use crate::models::AuthUser;
use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tracing::warn;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub uid: i64,
    pub user: String,
    pub exp: usize,
}

pub fn create_jwt(
    config: &AppConfig,
    user: &AuthUser,
) -> Result<String, Box<dyn std::error::Error>> {
    let expiration = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::days(30))
        .expect("valid timestamp")
        .timestamp() as usize;

    let claims = Claims {
        uid: user.uid,
        user: user.user.clone(),
        exp: expiration,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.jwt_secret.as_bytes()),
    )?;

    Ok(token)
}

pub struct AuthUserExtractor(pub AuthUser);

#[derive(Debug)]
pub struct AuthError(pub String);

impl axum::response::IntoResponse for AuthError {
    fn into_response(self) -> axum::response::Response {
        (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({
                "success": false,
                "message": self.0
            })),
        )
            .into_response()
    }
}

#[derive(Clone, Debug)]
struct LocalUser {
    name: String,
    uid: i64,
    home: String,
    shell: String,
}

fn passwd_users() -> Vec<LocalUser> {
    std::fs::read_to_string("/etc/passwd")
        .ok()
        .map(|contents| {
            contents
                .lines()
                .filter_map(|line| {
                    let mut parts = line.split(':');
                    let name = parts.next()?.to_string();
                    let _passwd = parts.next()?;
                    let uid = parts.next()?.parse::<i64>().ok()?;
                    let _gid = parts.next()?;
                    let _gecos = parts.next()?;
                    let home = parts.next()?.to_string();
                    let shell = parts.next()?.to_string();
                    Some(LocalUser {
                        name,
                        uid,
                        home,
                        shell,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn user_from_uid(uid: i64) -> Option<LocalUser> {
    passwd_users().into_iter().find(|user| user.uid == uid)
}

fn user_from_name(name: &str) -> Option<LocalUser> {
    passwd_users().into_iter().find(|user| user.name == name)
}

fn is_login_user(user: &LocalUser) -> bool {
    user.uid >= 1000
        && !matches!(user.name.as_str(), "nobody")
        && !user.shell.ends_with("/nologin")
        && !user.shell.ends_with("/false")
        && !user.shell.ends_with("/sync")
        && Path::new(&user.home).is_dir()
}

fn preferred_login_user() -> Option<LocalUser> {
    passwd_users()
        .into_iter()
        .filter(is_login_user)
        .min_by_key(|user| user.uid)
}

fn env_login_user() -> Option<LocalUser> {
    ["SUDO_USER", "LOGNAME", "USER"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .filter(|name| !name.is_empty() && name != "root")
        .find_map(|name| user_from_name(&name))
        .filter(is_login_user)
}

fn username_from_uid(uid: i64) -> Option<String> {
    user_from_uid(uid).map(|user| user.name)
}

fn fallback_real_user() -> Option<(String, i64)> {
    let uid = unsafe { libc::getuid() } as i64;

    if uid == 0 {
        if let Some(user) = env_login_user().or_else(preferred_login_user) {
            warn!(
                "系统头缺失，服务以 root 运行，自动锁定登录用户: {}",
                user.name
            );
            return Some((user.name, user.uid));
        }
    }

    let user = username_from_uid(uid)
        .or_else(|| std::env::var("USER").ok())
        .or_else(|| std::env::var("LOGNAME").ok())
        .unwrap_or_else(|| format!("uid-{}", uid));

    if !user.is_empty() {
        warn!("系统头缺失，使用当前进程身份: {}", user);
        return Some((user, uid));
    }

    None
}

pub fn home_for_user(username: &str) -> Option<String> {
    user_from_name(username).and_then(|user| {
        if user.home.is_empty() {
            None
        } else {
            Some(user.home)
        }
    })
}

/// 智能身份探测：优先 headers，其次使用当前进程身份兜底
///
/// x-fn-* 头只有在请求确实经由 fnOS 反向代理时才可信；服务同时也直接监听
/// 0.0.0.0，任何人都能伪造这些头。因此这里只把用户名当作"想切换到哪个账户"
/// 的提示，uid 一律以 /etc/passwd 为准，不接受调用方自报的 x-fn-uid ——
/// 否则调用方就能自行挑选 owner_uid，越权读取他人的会话与剪贴板。
pub fn detect_real_user(parts: &Parts) -> Option<(String, i64)> {
    // 1. 优先尝试从 fnOS 系统头获取身份 (系统级自动登录)
    let sys_user = parts
        .headers
        .get("x-fn-username")
        .or_else(|| parts.headers.get("x-fn-user"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if let Some(user) = sys_user {
        // 用户名必须对应真实本地账户，且必须是普通可登录账户（uid>=1000、
        // 有 home、shell 不是 nologin）：uid 取自 /etc/passwd 而不是请求头。
        //
        // 两条限制缺一不可。只查 passwd 不够 —— 调用方直接声明
        // `x-fn-username: root` 就能拿到 uid 0；而只信请求头里的 x-fn-uid，
        // 调用方等于可以随便挑 owner_uid，越权读别人的会话和剪贴板。
        match user_from_name(&user).filter(is_login_user) {
            Some(local_user) => return Some((local_user.name, local_user.uid)),
            None => {
                warn!(
                    "请求声明的系统用户 {} 不是本机普通登录账户，忽略该身份头",
                    user
                );
            }
        }
    }

    // 2. 兜底逻辑：本地直连或 fnOS 未透传系统头时，锁定真实可登录用户。
    fallback_real_user()
}

impl<S> FromRequestParts<S> for AuthUserExtractor
where
    S: Send + Sync,
{
    type Rejection = AuthError;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let config = parts.extensions.get::<Arc<AppConfig>>().cloned();

        let detected = detect_real_user(parts);

        let token = parts
            .headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|s| s.to_string())
            .or_else(|| {
                parts
                    .uri
                    .query()
                    .and_then(|q| q.split('&').find(|p| p.starts_with("token=")))
                    .map(|p| p.trim_start_matches("token=").to_string())
            });

        async move {
            let config = config.ok_or(AuthError("Server config not found".into()))?;

            // 1. 系统级自动登录 (带智能探测)
            //
            // 只有在压根没有配置密码时才允许免密。原先的条件是
            // `!has_password || has_skip_marker`，于是一个残留的 .skip_auth
            // 能直接击穿已设置的密码 —— 而 /api/auth/status 在这种状态下
            // 仍然返回 "login"，前端老老实实弹密码框，后面的 API 却是敞开的。
            if let Some((user, uid)) = detected {
                let has_password = std::fs::metadata(&config.auth_file).is_ok();

                if !has_password {
                    return Ok(AuthUserExtractor(AuthUser {
                        uid,
                        user,
                        is_admin: true,
                        groups: vec!["Users".into()],
                    }));
                }
            }

            // 2. Fallback 到 JWT 校验
            if let Some(t) = token {
                let token_data = decode::<Claims>(
                    &t,
                    &DecodingKey::from_secret(config.jwt_secret.as_bytes()),
                    &Validation::default(),
                )
                .map_err(|_| AuthError("Invalid token".into()))?;

                return Ok(AuthUserExtractor(AuthUser {
                    uid: token_data.claims.uid,
                    user: token_data.claims.user,
                    is_admin: true,
                    groups: vec!["Users".into()],
                }));
            }

            Err(AuthError("Authentication required".into()))
        }
    }
}

// 简单的密码哈希辅助
pub fn hash_password(pwd: &str) -> Result<String, bcrypt::BcryptError> {
    bcrypt::hash(pwd, bcrypt::DEFAULT_COST)
}

pub fn verify_password(pwd: &str, hash: &str) -> Result<bool, bcrypt::BcryptError> {
    bcrypt::verify(pwd, hash)
}
