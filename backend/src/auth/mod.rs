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
        warn!("使用当前进程身份: {}", user);
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

/// 智能身份探测：使用当前进程身份兜底
pub fn detect_real_user(_parts: &Parts) -> Option<(String, i64)> {
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

            // 1. 系统级自动登录 (基于本地用户探测)
            if let Some((user, uid)) = detected {
                let has_password = std::fs::metadata(&config.auth_file).is_ok();
                let has_skip_marker = std::fs::metadata(&config.skip_auth_file).is_ok();

                if !has_password || has_skip_marker {
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
