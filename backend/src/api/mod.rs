use axum::{
    extract::{
        ws::{Message, WebSocket},
        DefaultBodyLimit, Multipart, Path, State, WebSocketUpgrade,
    },
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde_json::Value;
use std::net::IpAddr;
use std::sync::Arc;
use tracing::{info, warn};

use crate::auth::*;
use crate::config::AppConfig;
use crate::db::DbPool;
use crate::models::*;
use crate::terminal;

/// 上传上限。axum 默认只有 2MiB，截图很容易超过而被静默拒绝；
/// 同时也不能不设上限，否则任何已鉴权调用方都能把数据盘写满。
const UPLOAD_SIZE_LIMIT: usize = 16 * 1024 * 1024;

// ─── App State ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub db: DbPool,
    pub sessions: terminal::SessionRegistry,
    pub attaches: terminal::AttachRegistry,
}

// ─── Router ─────────────────────────────────────────────────────────────

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .merge(api_routes())
        .nest("/app/rmux", api_routes())
        .with_state(state)
}

fn api_routes() -> Router<AppState> {
    Router::new()
        // App Auth (New Simplified Model)
        .route("/api/auth/status", get(get_auth_status))
        .route("/api/auth/setup", post(setup_password))
        .route("/api/auth/login", post(login_app))
        .route("/api/auth/me", get(get_me))
        // Sessions
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions", post(create_session))
        .route("/api/sessions/{session_id}", get(get_session))
        .route(
            "/api/sessions/{session_id}/name",
            patch(update_session_name),
        )
        .route("/api/sessions/{session_id}", delete(delete_session))
        // Terminal WebSocket
        .route("/ws/terminal/{session_id}", get(terminal_resume_ws))
        // Clipboard Bridge
        .route(
            "/api/terminal/clipboard",
            post(update_clipboard).layer(DefaultBodyLimit::max(UPLOAD_SIZE_LIMIT)),
        )
        // Clipboard History (global per-user, persisted in SQLite)
        .route("/api/clipboard", get(list_clipboard))
        .route("/api/clipboard", post(record_clipboard))
        .route("/api/clipboard", delete(clear_clipboard))
        // Theme
        .route("/api/theme", get(get_theme))
        .route("/api/theme", post(save_theme))
        .route("/api/theme/reset", post(reset_theme))
        .route(
            "/api/theme/background",
            post(upload_background).layer(DefaultBodyLimit::max(UPLOAD_SIZE_LIMIT)),
        )
        .route("/api/theme/background/{filename}", get(get_background_file))
        .route("/api/theme/background-url", post(save_background_url))
        // System
        .route("/api/system/info", get(get_system_info))
        .route("/api/system/purge", post(purge_data))
}

// ─── Auth Handlers ──────────────────────────────────────────────────────

async fn get_auth_status(State(state): State<AppState>) -> Json<ApiResponse<AppPasswordStatus>> {
    let has_password = std::fs::metadata(&state.config.auth_file).is_ok();
    let has_skip = std::fs::metadata(&state.config.skip_auth_file).is_ok();

    let status = if has_password {
        "login"
    } else if has_skip {
        "public"
    } else {
        "setup"
    };

    Json(ApiResponse::ok(AppPasswordStatus {
        status: status.into(),
    }))
}

async fn setup_password(
    State(state): State<AppState>,
    parts: axum::http::request::Parts,
    Json(req): Json<SetPasswordRequest>,
) -> Result<Json<ApiResponse<LoginResponse>>, ApiError> {
    if std::fs::metadata(&state.config.auth_file).is_ok()
        || std::fs::metadata(&state.config.skip_auth_file).is_ok()
    {
        return Err(ApiError::new("应用已完成初始化"));
    }

    let (username, uid) = detect_real_user(&parts)
        .ok_or_else(|| ApiError::new("无法识别系统用户身份，请通过桌面启动应用"))?;

    let user = AuthUser {
        uid,
        user: username,
        is_admin: true,
        groups: vec!["Users".into()],
    };

    if let Some(pwd) = req.password {
        if pwd.len() < 6 {
            return Err(ApiError::new("密码至少需要6位"));
        }
        let hash = hash_password(&pwd).map_err(|e| ApiError::new(e.to_string()))?;
        std::fs::write(&state.config.auth_file, hash).map_err(|e| ApiError::new(e.to_string()))?;
        info!("应用全局密码已设置");
    } else {
        // 创建跳过标记文件
        std::fs::write(&state.config.skip_auth_file, "skipped")
            .map_err(|e| ApiError::new(e.to_string()))?;
        info!("用户选择跳过密码设置，应用进入公开模式");
    }

    // 设置成功后，直接颁发 Token
    let token = create_jwt(&state.config, &user).map_err(|e| ApiError::new(e.to_string()))?;
    Ok(Json(ApiResponse::ok(LoginResponse { token, user })))
}

async fn login_app(
    State(state): State<AppState>,
    parts: axum::http::request::Parts,
    Json(req): Json<LoginRequest>,
) -> Result<Json<ApiResponse<LoginResponse>>, ApiError> {
    let raw_hash = std::fs::read_to_string(&state.config.auth_file)
        .map_err(|_| ApiError::new("尚未设置密码"))?;

    let hash = raw_hash.trim();

    let valid = verify_password(&req.password, hash)
        .map_err(|e| ApiError::new(format!("验证失败: {}", e)))?;

    if !valid {
        return Err(ApiError::new("密码错误"));
    }

    let (username, uid) = detect_real_user(&parts)
        .ok_or_else(|| ApiError::new("无法识别系统用户身份，请通过桌面启动应用"))?;

    let user = AuthUser {
        uid,
        user: username,
        is_admin: true,
        groups: vec!["Users".into()],
    };

    let token = create_jwt(&state.config, &user).map_err(|e| ApiError::new(e.to_string()))?;

    Ok(Json(ApiResponse::ok(LoginResponse { token, user })))
}

async fn get_me(parts: axum::http::request::Parts) -> Json<ApiResponse<AuthUser>> {
    if let Some((user, uid)) = detect_real_user(&parts) {
        Json(ApiResponse::ok(AuthUser {
            uid,
            user,
            is_admin: true,
            groups: vec!["Users".into()],
        }))
    } else {
        Json(ApiResponse {
            success: false,
            message: "未检测到系统登录信息".into(),
            data: None,
        })
    }
}

// ─── Session Handlers ───────────────────────────────────────────────────

/// 把会话注册表落盘。create / rename / delete 以及会话确认消失时调用。
///
/// resize 不落盘：尺寸在每次连接建立时都会被前端重新上报（首帧 resize 是
/// 协议约定），存的是旧值也无害，不值得为此每秒写盘。
///
/// 注意：内部会取读锁，调用前必须先放掉写锁，否则自锁。
async fn persist_sessions(state: &AppState) {
    terminal::persist_sessions(&state.config, &state.sessions).await;
}

async fn list_sessions(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
) -> Json<ApiResponse<Vec<SessionInfo>>> {
    let sessions = state.sessions.read().await;
    let owned_sessions: Vec<_> = sessions
        .values()
        .filter(|s| s.owner_uid == auth.0.uid)
        .cloned()
        .collect();
    drop(sessions);

    let mut list = Vec::new();
    for s in owned_sessions {
        let cwd =
            terminal::query_tmux_cwd(&state.config, &s.session_id, &s.tmux_session_name, &s.cwd)
                .await;
        list.push(SessionInfo {
            session_id: s.session_id.clone(),
            name: s.name.clone(),
            owner_uid: s.owner_uid,
            owner_user: s.owner_user.clone(),
            session_type: "local".into(),
            status: "active".into(),
            created_at: s.created_at.clone(),
            last_activity: s.last_activity.clone(),
            size: s.size.clone(),
            cwd,
        });
    }
    list.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Json(ApiResponse::ok(list))
}

async fn create_session(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<ApiResponse<SessionInfo>>, ApiError> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let session =
        terminal::create_local_session(&state.config, &session_id, &auth.0, req.cols, req.rows)
            .await
            .map_err(ApiError::new)?;

    let info = SessionInfo {
        session_id: session.session_id.clone(),
        name: session.name.clone(),
        owner_uid: session.owner_uid,
        owner_user: session.owner_user.clone(),
        session_type: "local".into(),
        status: "active".into(),
        created_at: session.created_at.clone(),
        last_activity: session.last_activity.clone(),
        size: session.size.clone(),
        cwd: session.cwd.clone(),
    };

    state
        .sessions
        .write()
        .await
        .insert(session.session_id.clone(), session);
    persist_sessions(&state).await;
    Ok(Json(ApiResponse::ok(info)))
}

async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    auth: AuthUserExtractor,
) -> Json<ApiResponse<Value>> {
    let sessions = state.sessions.read().await;
    if let Some(s) = sessions.get(&session_id) {
        if s.owner_uid != auth.0.uid {
            return Json(ApiResponse {
                success: false,
                message: "无权访问".into(),
                data: None,
            });
        }
        let session_id = s.session_id.clone();
        let session_name = s.tmux_session_name.clone();
        let fallback_cwd = s.cwd.clone();
        let session_name_for_response = s.name.clone();
        let size = s.size.clone();
        drop(sessions);
        let cwd =
            terminal::query_tmux_cwd(&state.config, &session_id, &session_name, &fallback_cwd)
                .await;
        Json(ApiResponse::ok(serde_json::json!({
            "session_id": session_id, "name": session_name_for_response, "size": size, "cwd": cwd,
        })))
    } else {
        Json(ApiResponse {
            success: false,
            message: "不存在".into(),
            data: None,
        })
    }
}

async fn update_session_name(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    auth: AuthUserExtractor,
    Json(req): Json<UpdateSessionNameRequest>,
) -> Result<Json<ApiResponse<SessionInfo>>, ApiError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::new("名称不能为空"));
    }
    if name.chars().count() > 32 {
        return Err(ApiError::new("名称不能超过32个字符"));
    }

    // 写锁只在这个块里活着 —— persist_sessions 会取读锁，带着写锁调用会自锁。
    let info = {
        let mut sessions = state.sessions.write().await;
        let session = sessions
            .get_mut(&session_id)
            .ok_or_else(|| ApiError::not_found("会话不存在"))?;

        if session.owner_uid != auth.0.uid {
            return Err(ApiError::forbidden("无权修改"));
        }

        session.name = name.to_string();
        session.last_activity = chrono::Utc::now().to_rfc3339();

        SessionInfo {
            session_id: session.session_id.clone(),
            name: session.name.clone(),
            owner_uid: session.owner_uid,
            owner_user: session.owner_user.clone(),
            session_type: session.session_type.clone(),
            status: "active".into(),
            created_at: session.created_at.clone(),
            last_activity: session.last_activity.clone(),
            size: session.size.clone(),
            cwd: session.cwd.clone(),
        }
    };

    persist_sessions(&state).await;
    Ok(Json(ApiResponse::ok(info)))
}

async fn delete_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    auth: AuthUserExtractor,
) -> Json<ApiResponse<Value>> {
    // 先在锁内完成校验和摘除，再放锁去做 tmux 子进程调用。
    // 原来是把写锁一直持到 kill_tmux_session().await 之后 —— 关一个标签页
    // 就会把其他所有注册表访问（含新的 WS attach）堵在一次子进程往返后面。
    let removed = {
        let mut sessions = state.sessions.write().await;
        match sessions.get(&session_id) {
            Some(s) if s.owner_uid != auth.0.uid => {
                return Json(ApiResponse {
                    success: false,
                    message: "无权删除".into(),
                    data: None,
                });
            }
            Some(_) => sessions.remove(&session_id),
            None => None,
        }
    };

    match removed {
        Some(s) => {
            // 先移除 attach 代号：关联 WebSocket 会从 changed() 分支退出，
            // PtyControl 随之析构并杀掉 attach 子进程，读/写/wait 三个线程也会收到
            // EOF、通道关闭或子进程退出。之后再杀持久 tmux 会话，销毁顺序更确定。
            terminal::release_attach(&state.attaches, &session_id).await;
            let _ =
                terminal::kill_tmux_session(&state.config, &session_id, &s.tmux_session_name, true)
                    .await;
            persist_sessions(&state).await;
            Json(ApiResponse::ok(serde_json::json!({})))
        }
        None => Json(ApiResponse {
            success: false,
            message: "不存在".into(),
            data: None,
        }),
    }
}

// ─── WebSocket Handlers ──────────────────────────────────────────────────

async fn terminal_resume_ws(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    Path(session_id): Path<String>,
    auth: AuthUserExtractor,
) -> Response {
    ws.on_upgrade(move |socket| handle_terminal_resume(state, socket, session_id, auth))
}

/// 前端连上后多久还没报告尺寸就按记录值开工，避免连接卡死。
const INITIAL_SIZE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(3000);
/// 空闲连接会被反向代理按读超时掐断，而两端都不会收到通知：终端看着还在，
/// 输出却永远不再送达。定期 Ping 既保活也能尽早发现死连接。
const WS_PING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);
/// 浏览器 xterm 与跨设备恢复都保留最近 5000 行，避免两个历史上限不一致。
const TERMINAL_HISTORY_LINES: usize = 5000;

async fn forget_gone_session(state: &AppState, session_id: &str) {
    let removed = state.sessions.write().await.remove(session_id).is_some();
    terminal::release_attach(&state.attaches, session_id).await;
    if removed {
        persist_sessions(state).await;
    }
}

/// tmux 一 attach 就会按当前 PTY 尺寸整屏重绘。若此时浏览器还没报告真实尺寸，
/// 这一屏就画在错误的网格上，而之后 tmux 只发增量更新 —— 基线错了就再也回不来，
/// 残影、上下叠帧、光标错位全部由此而来。所以先收第一条 resize 再 attach。
///
/// 这期间到达的按键先攒着，attach 之后原样补发，避免丢输入。
async fn await_initial_size(
    ws: &mut WebSocket,
    fallback: &TermSize,
    pending_input: &mut Vec<Vec<u8>>,
) -> Option<TermSize> {
    let deadline = tokio::time::Instant::now() + INITIAL_SIZE_TIMEOUT;
    loop {
        match tokio::time::timeout_at(deadline, ws.recv()).await {
            // 前端没按约定报尺寸（老版本页面），退回记录值继续。
            Err(_) => return Some(fallback.clone()),
            Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => return None,
            Ok(Some(Ok(Message::Text(text)))) => {
                match serde_json::from_str::<TerminalMessage>(&text) {
                    Ok(TerminalMessage::Resize { cols, rows }) => {
                        return Some(TermSize { cols, rows })
                    }
                    Ok(TerminalMessage::Input { data }) => pending_input.push(data.into_bytes()),
                    _ => {}
                }
            }
            Ok(Some(Ok(Message::Binary(bytes)))) => pending_input.push(bytes.to_vec()),
            Ok(Some(Ok(_))) => {}
        }
    }
}

async fn handle_terminal_resume(
    state: AppState,
    mut ws: WebSocket,
    session_id: String,
    auth: AuthUserExtractor,
) {
    let (tmux_name, owner, stored_size) = {
        let sessions = state.sessions.read().await;
        match sessions.get(&session_id) {
            // 归属校验：owner 直接决定 attach 时 runuser/su 切到哪个账户，
            // 服务又以 root 运行，漏掉这一步等于任何通过鉴权的调用方
            // 只要猜到 session_id 就能拿到该账户的交互式 shell。
            // 越权时回 SESSION_NOT_FOUND 而不是"无权"，避免探测会话是否存在。
            Some(s) if s.owner_uid != auth.0.uid => {
                warn!(
                    "拒绝越权 attach: session={} owner_uid={} requester_uid={}",
                    session_id, s.owner_uid, auth.0.uid
                );
                let _ = ws.send(Message::Text("SESSION_NOT_FOUND".into())).await;
                return;
            }
            Some(s) => (
                s.tmux_session_name.clone(),
                AuthUser {
                    uid: s.owner_uid,
                    user: s.owner_user.clone(),
                    is_admin: true,
                    groups: vec!["Users".into()],
                },
                s.size.clone(),
            ),
            None => {
                let _ = ws.send(Message::Text("SESSION_NOT_FOUND".into())).await;
                return;
            }
        }
    };

    let mut pending_input = Vec::new();
    let Some(size) = await_initial_size(&mut ws, &stored_size, &mut pending_input).await else {
        return;
    };

    // 同一条 tmux command queue 先验证 session，再刷新浏览器相关选项。若会话已经
    // 退出，这里会直接失败，不会像旧实现那样由 set-option 启动空 server，随后把
    // `no sessions` 输出到终端。
    if let Err(e) = terminal::prepare_tmux_session(&state.config, &session_id, &tmux_name).await {
        warn!("准备 tmux 会话失败: {}", e);
        if !terminal::check_tmux_session(&state.config, &session_id, &tmux_name).await {
            forget_gone_session(&state, &session_id).await;
            let _ = ws.send(Message::Text("SESSION_GONE".into())).await;
            return;
        }
    }

    // 登记要放在 attach 之前：attach 带 `-d`，会顶掉上一个客户端，
    // 上一个必须先收到"被接管"的通知，才不会转头又重连回来。
    let mut replaced = terminal::claim_attach(&state.attaches, &session_id).await;

    // 新浏览器没有旧 xterm buffer。先从 tmux 抓取当前屏幕之前的持久历史；attach
    // 随后只负责重绘当前屏幕，两部分按 WebSocket 顺序拼起来即可完整恢复。
    let history = match terminal::capture_tmux_history(
        &state.config,
        &session_id,
        &tmux_name,
        TERMINAL_HISTORY_LINES,
        clamp_axis(size.rows),
    )
    .await
    {
        Ok(history) => history,
        Err(e) => {
            warn!("恢复 tmux 历史失败: {}", e);
            if !terminal::check_tmux_session(&state.config, &session_id, &tmux_name).await {
                forget_gone_session(&state, &session_id).await;
                let _ = ws.send(Message::Text("SESSION_GONE".into())).await;
                return;
            }
            Vec::new()
        }
    };

    let terminal::PtyIo {
        mut output,
        control,
    } = match terminal::attach_pty_session(
        &state.config,
        &session_id,
        &tmux_name,
        &owner,
        clamp_axis(size.cols),
        clamp_axis(size.rows),
    ) {
        Ok(io) => io,
        Err(e) => {
            warn!("attach 终端失败: {}", e);
            // claim 已经把上一个客户端踢下线了，而我们自己又没接上。
            // 不回滚的话：上一个窗口永久停在"已在其它窗口打开"，这个窗口停在
            // SESSION_GONE，而 tmux 会话其实一直好好活着，只能整页刷新才能救回来。
            terminal::release_attach(&state.attaches, &session_id).await;
            let _ = ws.send(Message::Text("SESSION_GONE".into())).await;
            return;
        }
    };

    if let Some(s) = state.sessions.write().await.get_mut(&session_id) {
        s.size = size;
        s.last_activity = chrono::Utc::now().to_rfc3339();
    }

    // 必须在读取 live PTY queue 前发历史快照，WebSocket 才能保证客户端先建立
    // scrollback、再处理 tmux 的清屏和当前屏幕重绘。
    if !history.is_empty() && ws.send(Message::Binary(history.into())).await.is_err() {
        return;
    }

    for data in pending_input {
        if !control.write_input(data) {
            return;
        }
    }

    let mut ping = tokio::time::interval(WS_PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await; // interval 的第一拍是立即触发的，丢掉。

    loop {
        tokio::select! {
            _ = ping.tick() => {
                if ws.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            // changed() 出错说明发送端没了 —— 那是会话被删除，不是被接管，
            // 此时不能报 SESSION_REPLACED，否则用户主动关标签也会看到"已在其它窗口打开"。
            outcome = replaced.changed() => {
                if outcome.is_ok() {
                    let _ = ws.send(Message::Text("SESSION_REPLACED".into())).await;
                }
                break;
            }
            chunk = output.recv() => {
                match chunk {
                    // tmux 生成的字节原样转发。必须走二进制帧：按 UTF-8 解码会在
                    // 分块边界切断多字节字符，中文和框线字符会碎成 U+FFFD。
                    Some(bytes) => {
                        if ws.send(Message::Binary(bytes.into())).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        // PTY 读到 EOF：tmux 客户端退出了。可能是会话真的结束，
                        // 也可能只是被同会话的新客户端顶掉，两者要区别对待。
                        // 新 attach 会让旧 tmux client 先 EOF，watch 通知也几乎同时到；
                        // select 若抢先选到 EOF，仍要检查代号，不能让旧浏览器自动重连
                        // 后再次顶掉新电脑。
                        if replaced.has_changed().unwrap_or(false) {
                            let _ = ws.send(Message::Text("SESSION_REPLACED".into())).await;
                        } else if !terminal::check_tmux_session(&state.config, &session_id, &tmux_name).await {
                            forget_gone_session(&state, &session_id).await;
                            let _ = ws.send(Message::Text("SESSION_GONE".into())).await;
                        }
                        break;
                    }
                }
            }
            msg = ws.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(cmd) = serde_json::from_str::<TerminalMessage>(&text) else {
                            continue;
                        };
                        match cmd {
                            TerminalMessage::Input { data } => {
                                if !control.write_input(data.into_bytes()) {
                                    break;
                                }
                            }
                            TerminalMessage::Paste { data } => {
                                if let Err(e) = terminal::paste_to_tmux(&state.config, &session_id, &tmux_name, &data).await {
                                    warn!("粘贴失败: {}", e);
                                }
                            }
                            TerminalMessage::Resize { cols, rows } => {
                                control.resize(clamp_axis(cols), clamp_axis(rows));
                                if let Some(s) = state.sessions.write().await.get_mut(&session_id) {
                                    s.size = TermSize { cols, rows };
                                    s.last_activity = chrono::Utc::now().to_rfc3339();
                                }
                            }
                            TerminalMessage::CloseTerminal => break,
                            TerminalMessage::ResumeSession { .. } | TerminalMessage::KeepAlive => {}
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        if !control.write_input(bytes.to_vec()) {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => { warn!("WS error: {}", e); break; }
                    _ => {}
                }
            }
        }
    }
}

/// tmux 对异常尺寸容错很差，钳到可用范围再传下去。
fn clamp_axis(value: u32) -> u16 {
    value.clamp(1, 1000) as u16
}

// ─── Clipboard & Theme & System Handlers ────────────────────────────────

async fn update_clipboard(
    State(state): State<AppState>,
    _auth: AuthUserExtractor,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    let mut files = Vec::new();

    while let Some(field) = multipart.next_field().await.unwrap_or(None) {
        let field_name = field.name().unwrap_or("data").to_string();
        let content_type = field.content_type().map(|s| s.to_string());
        let original_name = field.file_name().map(|s| s.to_string());
        let data = field
            .bytes()
            .await
            .map_err(|e| ApiError::new(e.to_string()))?;
        let ext = clipboard_extension(content_type.as_deref(), original_name.as_deref());
        let name = format!("paste-{}.{}", uuid::Uuid::new_v4(), ext);
        let path = state.config.clipboard_dir.join(&name);
        tokio::fs::write(&path, &data)
            .await
            .map_err(|e| ApiError::new(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // 必须用阻塞版：tokio::fs::set_permissions 返回 Future，
            // `let _ =` 会把它直接丢弃，权限根本不会被改。文件由 root 写入，
            // umask 077 时会落成 0600，被 runuser 切过去的用户 shell 读不到
            // 刚刚粘贴进来的图片 —— 招牌功能静默失效。
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666));
        }
        files.push(serde_json::json!({
            "field": field_name,
            "path": path.to_string_lossy(),
            "contentType": content_type,
            "size": data.len(),
        }));
    }

    Ok(Json(ApiResponse::ok(serde_json::json!({ "files": files }))))
}

fn clipboard_extension(content_type: Option<&str>, file_name: Option<&str>) -> &'static str {
    match content_type
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        _ => file_name
            .and_then(|name| name.rsplit('.').next())
            .and_then(|ext| match ext.to_ascii_lowercase().as_str() {
                "png" => Some("png"),
                "jpg" | "jpeg" => Some("jpg"),
                "gif" => Some("gif"),
                "webp" => Some("webp"),
                "bmp" => Some("bmp"),
                "tif" | "tiff" => Some("tiff"),
                _ => None,
            })
            .unwrap_or("bin"),
    }
}

// ─── Clipboard History (per-user, global across sessions/browsers) ──────

const CLIPBOARD_HISTORY_LIMIT: i64 = 20;

fn remove_clipboard_image_files(state: &AppState, paths: Vec<String>) {
    for path in paths {
        let file_path = std::path::Path::new(&path);
        if file_path.starts_with(&state.config.clipboard_dir) {
            let _ = std::fs::remove_file(file_path);
        }
    }
}

fn prune_clipboard_history(
    state: &AppState,
    conn: &rusqlite::Connection,
    owner_uid: i64,
) -> Result<(), ApiError> {
    let stale_paths = {
        let mut stmt = conn
            .prepare(
                "SELECT path FROM clipboard_history
                 WHERE owner_uid=?1 AND kind='image' AND path IS NOT NULL
                   AND id NOT IN (
                     SELECT id FROM clipboard_history
                     WHERE owner_uid=?1
                     ORDER BY created_at DESC
                     LIMIT ?2
                   )",
            )
            .map_err(|e| ApiError::new(e.to_string()))?;
        let rows = stmt
            .query_map(
                rusqlite::params![owner_uid, CLIPBOARD_HISTORY_LIMIT],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| ApiError::new(e.to_string()))?;
        rows.filter_map(Result::ok).collect::<Vec<_>>()
    };

    conn.execute(
        "DELETE FROM clipboard_history
         WHERE owner_uid=?1 AND id NOT IN (
             SELECT id FROM clipboard_history WHERE owner_uid=?1 ORDER BY created_at DESC LIMIT ?2
         )",
        rusqlite::params![owner_uid, CLIPBOARD_HISTORY_LIMIT],
    )
    .map_err(|e| ApiError::new(e.to_string()))?;

    remove_clipboard_image_files(state, stale_paths);
    Ok(())
}

async fn list_clipboard(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
) -> Result<Json<ApiResponse<Vec<ClipboardItem>>>, ApiError> {
    let conn = state.db.get().map_err(|e| ApiError::new(e.to_string()))?;
    prune_clipboard_history(&state, &conn, auth.0.uid)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, text, path, content_type, size, created_at
             FROM clipboard_history
             WHERE owner_uid=?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )
        .map_err(|e| ApiError::new(e.to_string()))?;
    let rows = stmt
        .query_map(
            rusqlite::params![auth.0.uid, CLIPBOARD_HISTORY_LIMIT],
            |row| {
                Ok(ClipboardItem {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    text: row.get(2)?,
                    path: row.get(3)?,
                    content_type: row.get(4)?,
                    size: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .map_err(|e| ApiError::new(e.to_string()))?;

    let mut items: Vec<ClipboardItem> = Vec::new();
    for item in rows.flatten() {
        // 过滤掉本地文件已被清理的图片项，避免点击后粘贴一个失效路径
        if item.kind == "image" {
            if let Some(p) = item.path.as_deref() {
                if !std::path::Path::new(p).exists() {
                    let _ = conn.execute(
                        "DELETE FROM clipboard_history WHERE id=?1",
                        rusqlite::params![item.id],
                    );
                    continue;
                }
            }
        }
        items.push(item);
    }
    Ok(Json(ApiResponse::ok(items)))
}

async fn record_clipboard(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
    Json(req): Json<RecordClipboardRequest>,
) -> Result<Json<ApiResponse<ClipboardItem>>, ApiError> {
    let kind = req.kind.as_str();
    if kind != "text" && kind != "image" {
        return Err(ApiError::new("kind 必须为 text 或 image"));
    }
    if kind == "text" && req.text.as_deref().map(str::is_empty).unwrap_or(true) {
        return Err(ApiError::new("文本内容为空"));
    }
    if kind == "image" && req.path.as_deref().map(str::is_empty).unwrap_or(true) {
        return Err(ApiError::new("图片路径为空"));
    }

    let conn = state.db.get().map_err(|e| ApiError::new(e.to_string()))?;

    // 去重：相同 owner + 相同内容的旧记录先移除，新写入会自然冒到最上
    match kind {
        "text" => {
            let _ = conn.execute(
                "DELETE FROM clipboard_history WHERE owner_uid=?1 AND kind='text' AND text=?2",
                rusqlite::params![auth.0.uid, req.text],
            );
        }
        "image" => {
            let _ = conn.execute(
                "DELETE FROM clipboard_history WHERE owner_uid=?1 AND kind='image' AND path=?2",
                rusqlite::params![auth.0.uid, req.path],
            );
        }
        _ => {}
    }

    let id = uuid::Uuid::new_v4().to_string();
    let created_at = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO clipboard_history (id, owner_uid, kind, text, path, content_type, size, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            id,
            auth.0.uid,
            req.kind,
            req.text,
            req.path,
            req.content_type,
            req.size,
            created_at,
        ],
    )
    .map_err(|e| ApiError::new(e.to_string()))?;

    prune_clipboard_history(&state, &conn, auth.0.uid)?;

    Ok(Json(ApiResponse::ok(ClipboardItem {
        id,
        kind: req.kind,
        text: req.text,
        path: req.path,
        content_type: req.content_type,
        size: req.size,
        created_at,
    })))
}

async fn clear_clipboard(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    let conn = state.db.get().map_err(|e| ApiError::new(e.to_string()))?;
    let image_paths = {
        let mut stmt = conn
            .prepare(
                "SELECT path FROM clipboard_history
                 WHERE owner_uid=?1 AND kind='image' AND path IS NOT NULL",
            )
            .map_err(|e| ApiError::new(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![auth.0.uid], |row| row.get::<_, String>(0))
            .map_err(|e| ApiError::new(e.to_string()))?;
        rows.filter_map(Result::ok).collect::<Vec<_>>()
    };
    conn.execute(
        "DELETE FROM clipboard_history WHERE owner_uid=?1",
        rusqlite::params![auth.0.uid],
    )
    .map_err(|e| ApiError::new(e.to_string()))?;
    remove_clipboard_image_files(&state, image_paths);
    Ok(Json(ApiResponse::ok(serde_json::json!({}))))
}

/// 主题默认值。`ThemeSettings` 已按 camelCase 序列化，与命中分支同构。
fn theme_defaults_json() -> Value {
    serde_json::to_value(ThemeSettings::default()).unwrap_or_else(|_| serde_json::json!({}))
}

async fn get_theme(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    // 这里原来对 pool.get() 和 prepare() 双 unwrap —— 全库唯一一个能把
    // 活跃请求 panic 掉的 handler（连接池耗尽即触发）。
    let conn = state
        .db
        .get()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut stmt = conn.prepare("SELECT theme, font_size, background_opacity, background_image, background_image_type, saved_url_image, saved_upload_image, tab_style, cursor_style, component_customization FROM theme_settings WHERE owner_uid=?1 AND is_active=1 ORDER BY id DESC LIMIT 1").map_err(|e| ApiError::internal(e.to_string()))?;
    let result = stmt.query_row([auth.0.uid], |row| {
        Ok(serde_json::json!({
            "theme": row.get::<_, String>(0)?, "fontSize": row.get::<_, i64>(1)?, "backgroundOpacity": row.get::<_, i64>(2)?,
            "backgroundImage": row.get::<_, String>(3)?, "backgroundImageType": row.get::<_, String>(4)?, "savedUrlImage": row.get::<_, String>(5)?,
            "savedUploadImage": row.get::<_, String>(6)?, "tabStyle": row.get::<_, String>(7)?, "cursorStyle": row.get::<_, String>(8)?,
            "componentCustomization": serde_json::from_str::<Value>(&row.get::<_, String>(9)?).unwrap_or_default(),
        }))
    });
    match result {
        Ok(settings) => Ok(Json(ApiResponse::ok(settings))),
        Err(_) => Ok(Json(ApiResponse::ok(theme_defaults_json()))),
    }
}

async fn save_theme(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
    Json(body): Json<Value>,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    let conn = state
        .db
        .get()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    // 原先只是把旧行置 is_active=0 再插新行，而查询只看 is_active=1 —— 旧行
    // 永远读不到却永远留着，表随保存次数无界增长。直接删掉等价且有界。
    conn.execute(
        "DELETE FROM theme_settings WHERE owner_uid=?1",
        [auth.0.uid],
    )
    .ok();
    conn.execute("INSERT INTO theme_settings (owner_uid, owner_user, theme, font_size, background_opacity, background_image, background_image_type, saved_url_image, saved_upload_image, tab_style, cursor_style, component_customization, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![auth.0.uid, auth.0.user, body.get("theme").and_then(|v| v.as_str()).unwrap_or("onedark"), body.get("fontSize").and_then(|v| v.as_i64()).unwrap_or(14), body.get("backgroundOpacity").and_then(|v| v.as_i64()).unwrap_or(0), body.get("backgroundImage").and_then(|v| v.as_str()).unwrap_or(""), body.get("backgroundImageType").and_then(|v| v.as_str()).unwrap_or("none"), body.get("savedUrlImage").and_then(|v| v.as_str()).unwrap_or(""), body.get("savedUploadImage").and_then(|v| v.as_str()).unwrap_or(""), body.get("tabStyle").and_then(|v| v.as_str()).unwrap_or("modern"), body.get("cursorStyle").and_then(|v| v.as_str()).unwrap_or("block"), &body.get("componentCustomization").map(|v| v.to_string()).unwrap_or_default(), now]).map_err(|e| ApiError::new(e.to_string()))?;
    Ok(Json(ApiResponse::ok(serde_json::json!({}))))
}

async fn reset_theme(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    let conn = state
        .db
        .get()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    conn.execute(
        "DELETE FROM theme_settings WHERE owner_uid=?1",
        [auth.0.uid],
    )
    .ok();
    Ok(Json(ApiResponse::ok(theme_defaults_json())))
}

async fn upload_background(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    // 只取第一个字段。原先写成 `while let`，循环体无条件 return，
    // 触发 clippy 的 deny-by-default `never_loop`；语义本就是"取一个"。
    let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::new(e.to_string()))?
    else {
        return Err(ApiError::new("未收到文件"));
    };

    let data = field
        .bytes()
        .await
        .map_err(|e| ApiError::new(e.to_string()))?;
    let filename = format!("{}_{}.png", auth.0.uid, chrono::Utc::now().timestamp());
    let path = state.config.backgrounds_dir.join(&filename);
    tokio::fs::write(&path, &data)
        .await
        .map_err(|e| ApiError::new(e.to_string()))?;
    Ok(Json(ApiResponse::ok(
        serde_json::json!({ "filename": filename }),
    )))
}

/// 把路径参数收敛成 backgrounds 目录下的单个普通文件名。
///
/// axum 会对路径参数做百分号解码，因此 `%2e%2e%2f` 到达这里就是真正的 `../`；
/// 直接 join 到目录上就是一个任意文件读原语。这里要求整个参数正好是一段
/// Normal 组件：带分隔符、`..`、根前缀或空的一律拒绝。
fn resolve_background_path(config: &AppConfig, filename: &str) -> Option<std::path::PathBuf> {
    let mut components = std::path::Path::new(filename).components();
    let name = match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(name)), None) => name,
        _ => return None,
    };
    if name.to_str().map(|s| s.starts_with('.')).unwrap_or(true) {
        return None;
    }
    Some(config.backgrounds_dir.join(name))
}

async fn get_background_file(
    State(state): State<AppState>,
    _auth: AuthUserExtractor,
    Path(filename): Path<String>,
) -> Result<Response, StatusCode> {
    let path = resolve_background_path(&state.config, &filename).ok_or(StatusCode::NOT_FOUND)?;
    match tokio::fs::read(&path).await {
        Ok(data) => Ok(([(header::CONTENT_TYPE, "image/png")], data).into_response()),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

/// 背景图下载上限，同时也是 URL 下载的读取上限。
const BACKGROUND_URL_LIMIT: usize = 16 * 1024 * 1024;

/// 回环与链路本地地址一律拒绝：前者能打到本机上任何只监听 127.0.0.1 的服务
/// （包括 Rmux 自己），后者覆盖 169.254.169.254 这类云元数据端点。
///
/// 私网段（192.168/10/172.16）不拦 —— NAS 本来就在局域网里，从同网段主机取
/// 一张壁纸是正常用法，拦掉会误伤。
fn is_blocked_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_multicast()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fe80::/10 链路本地
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                // ::ffff:127.0.0.1 之类的映射地址要按 IPv4 规则再判一次
                || v6
                    .to_ipv4_mapped()
                    .map(|v4| is_blocked_ip(&IpAddr::V4(v4)))
                    .unwrap_or(false)
        }
    }
}

async fn fetch_remote_image(url: &str) -> Result<Vec<u8>, ApiError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| ApiError::new("URL 格式无效"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ApiError::new("仅支持 http/https 链接"));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| ApiError::new("URL 缺少主机名"))?
        .to_string();
    let port = parsed.port_or_known_default().unwrap_or(80);

    // 逐个检查解析结果：只看字面量会被"域名指向 127.0.0.1"直接绕过。
    let addrs = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| ApiError::new("无法解析该主机"))?
        .collect::<Vec<_>>();
    if addrs.is_empty() {
        return Err(ApiError::new("无法解析该主机"));
    }
    if addrs.iter().any(|addr| is_blocked_ip(&addr.ip())) {
        return Err(ApiError::forbidden("不允许访问该地址"));
    }

    let client = reqwest::Client::builder()
        // 跳转会绕过上面刚做完的地址检查，直接禁掉。
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let mut resp = client
        .get(parsed)
        .send()
        .await
        .map_err(|e| ApiError::new(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(ApiError::new(format!("下载失败: HTTP {}", resp.status())));
    }

    // Content-Length 是对方说了算的，边收边计数才是真的上限。
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| ApiError::new(e.to_string()))?
    {
        if body.len() + chunk.len() > BACKGROUND_URL_LIMIT {
            return Err(ApiError::new("图片超过 16MB 限制"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn save_background_url(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
    Json(body): Json<Value>,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    let url = body
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::new("缺少 url"))?;
    let data = fetch_remote_image(url).await?;
    let filename = format!("{}_{}_url.png", auth.0.uid, chrono::Utc::now().timestamp());
    tokio::fs::write(state.config.backgrounds_dir.join(&filename), &data)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(ApiResponse::ok(
        serde_json::json!({ "filename": filename }),
    )))
}

async fn get_system_info(_auth: AuthUserExtractor) -> Json<ApiResponse<Value>> {
    let hostname =
        std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_else(|_| "unknown".into());
    Json(ApiResponse::ok(
        serde_json::json!({ "hostname": hostname.trim(), "os": "Linux", "arch": std::env::consts::ARCH }),
    ))
}

async fn purge_data(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    // 危险操作：清空当前用户的数据 —— 背景图、主题、剪贴板历史及其图片文件，
    // 外加全局的应用日志目录。
    // 明确不做的事：不动 rmux.db 以外的凭据文件，不删正在使用的 tmux socket。
    //
    // 背景图按 `<uid>_` 前缀归属，只删自己的 —— 原先是整个目录 remove_dir_all，
    // 任何一个已鉴权用户都能顺手抹掉其他人的背景图。
    let _ = std::fs::remove_dir_all(&state.config.logs_dir);

    let prefix = format!("{}_", auth.0.uid);
    if let Ok(entries) = std::fs::read_dir(&state.config.backgrounds_dir) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_str()
                .map(|name| name.starts_with(&prefix))
                .unwrap_or(false)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    let conn = state
        .db
        .get()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let image_paths = {
        let mut stmt = conn
            .prepare(
                "SELECT path FROM clipboard_history
                 WHERE owner_uid=?1 AND kind='image' AND path IS NOT NULL",
            )
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![auth.0.uid], |row| row.get::<_, String>(0))
            .map_err(|e| ApiError::internal(e.to_string()))?;
        rows.filter_map(Result::ok).collect::<Vec<_>>()
    };
    let _ = conn.execute(
        "DELETE FROM clipboard_history WHERE owner_uid=?1",
        rusqlite::params![auth.0.uid],
    );
    let _ = conn.execute(
        "DELETE FROM theme_settings WHERE owner_uid=?1",
        rusqlite::params![auth.0.uid],
    );
    remove_clipboard_image_files(&state, image_paths);

    let _ = state.config.ensure_dirs();
    Ok(Json(ApiResponse::ok(serde_json::json!({}))))
}
