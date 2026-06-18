use axum::{
    extract::{
        ws::{Message, WebSocket},
        Multipart, Path, State, WebSocketUpgrade,
    },
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tracing::{info, warn};

use crate::auth::*;
use crate::config::AppConfig;
use crate::db::DbPool;
use crate::models::*;
use crate::terminal;

// ─── App State ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub db: DbPool,
    pub sessions: terminal::SessionRegistry,
}

const INPUT_SNAPSHOT_DELAY_MS: u64 = 15;
const ENTER_SNAPSHOT_DELAY_MS: u64 = 80;

// ─── Router ─────────────────────────────────────────────────────────────

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .merge(api_routes())
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
        .route("/api/terminal/clipboard", post(update_clipboard))
        // Clipboard History (global per-user, persisted in SQLite)
        .route("/api/clipboard", get(list_clipboard))
        .route("/api/clipboard", post(record_clipboard))
        .route("/api/clipboard", delete(clear_clipboard))
        // Theme
        .route("/api/theme", get(get_theme))
        .route("/api/theme", post(save_theme))
        .route("/api/theme/reset", post(reset_theme))
        .route("/api/theme/background", post(upload_background))
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
    state
        .config
        .ensure_dirs()
        .map_err(|e| ApiError::new(e.to_string()))?;

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
            .map_err(|e| ApiError::new(e))?;

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

    let mut sessions = state.sessions.write().await;
    sessions.insert(session.session_id.clone(), session);
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

    let mut sessions = state.sessions.write().await;
    let session = sessions
        .get_mut(&session_id)
        .ok_or_else(|| ApiError::new("会话不存在"))?;

    if session.owner_uid != auth.0.uid {
        return Err(ApiError::new("无权修改"));
    }

    session.name = name.to_string();
    session.last_activity = chrono::Utc::now().to_rfc3339();

    let info = SessionInfo {
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
    };

    Ok(Json(ApiResponse::ok(info)))
}

async fn delete_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    auth: AuthUserExtractor,
) -> Json<ApiResponse<Value>> {
    let mut sessions = state.sessions.write().await;
    if let Some(s) = sessions.get(&session_id) {
        if s.owner_uid != auth.0.uid {
            return Json(ApiResponse {
                success: false,
                message: "无权删除".into(),
                data: None,
            });
        }
    }
    if let Some(s) = sessions.remove(&session_id) {
        let _ = terminal::kill_tmux_session(&state.config, &session_id, &s.tmux_session_name, true)
            .await;
        Json(ApiResponse::ok(serde_json::json!({})))
    } else {
        Json(ApiResponse {
            success: false,
            message: "不存在".into(),
            data: None,
        })
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

async fn handle_terminal_resume(
    state: AppState,
    mut ws: WebSocket,
    session_id_param: String,
    auth: AuthUserExtractor,
) {
    let session = {
        let sessions = state.sessions.read().await;
        sessions.get(&session_id_param).cloned()
    };
    let session = if let Some(session) = session {
        session
    } else if let Some(session) =
        recover_tmux_session(&state.config, &session_id_param, &auth.0).await
    {
        let mut sessions = state.sessions.write().await;
        sessions
            .entry(session_id_param.clone())
            .or_insert_with(|| session.clone());
        session
    } else {
        let _ = ws.send(Message::Text("SESSION_NOT_FOUND".into())).await;
        return;
    };

    let tmux_name = session.tmux_session_name.clone();
    let output_file = session.output_file.clone();
    let is_new = session.is_new;

    if is_new {
        let mut sessions = state.sessions.write().await;
        if let Some(s) = sessions.get_mut(&session_id_param) {
            s.is_new = false;
        }
    }

    let config_clone = state.config.clone();
    let session_id_param_clone = session_id_param.clone();
    let tmux_name_clone = tmux_name.clone();
    let mut history_sent = false;

    const MAX_OUTPUT_DELTA_BYTES: u64 = 256 * 1024;
    const OUTPUT_POLL_INTERVAL_MS: u64 = 30;

    let mut output_file_handle = match tokio::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(&output_file)
        .await
    {
        Ok(file) => Some(file),
        Err(e) => {
            warn!("打开输出文件失败: {}", e);
            None
        }
    };
    let mut last_output_size = if let Some(file) = output_file_handle.as_mut() {
        file.metadata().await.map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    let mut output_poll = tokio::time::interval(tokio::time::Duration::from_millis(
        OUTPUT_POLL_INTERVAL_MS,
    ));

    'connection: loop {
        tokio::select! {
            _ = output_poll.tick() => {
                let Some(file) = output_file_handle.as_mut() else {
                    continue;
                };

                if let Ok(meta) = file.metadata().await {
                    let len = meta.len();
                    if len < last_output_size {
                        last_output_size = 0;
                        let _ = file.seek(SeekFrom::Start(0)).await;
                    }

                    while len > last_output_size {
                        let read_len = std::cmp::min(len - last_output_size, MAX_OUTPUT_DELTA_BYTES);
                        let mut buffer = vec![0; read_len as usize];
                        let _ = file.seek(SeekFrom::Start(last_output_size)).await;
                        if file.read_exact(&mut buffer).await.is_err() {
                            break;
                        }

                        let delta = String::from_utf8_lossy(&buffer).to_string();
                        last_output_size += read_len;
                        if ws.send(Message::Text(delta.into())).await.is_err() {
                            break 'connection;
                        }
                    }
                }
            }
            msg = ws.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(cmd) = serde_json::from_str::<TerminalMessage>(&text) {
                            match cmd {
                                TerminalMessage::Input { data } => {
                                    let _ = terminal::write_to_tmux(&state.config, &session_id_param, &tmux_name, &data).await;
                                    if let Some(delay_ms) = input_snapshot_delay_ms(&data) {
                                        let submitted = is_submit_input(&data);
                                        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                                        if terminal::should_snapshot_after_input(&config_clone, &session_id_param_clone, &tmux_name_clone, submitted).await {
                                            if !send_terminal_snapshot(&mut ws, &config_clone, &session_id_param_clone, &tmux_name_clone, 500).await {
                                                break;
                                            }
                                            skip_captured_output(&mut output_file_handle, &mut last_output_size).await;
                                        }
                                    }
                                }
                                TerminalMessage::Paste { data } => {
                                    let _ = terminal::paste_to_tmux(&state.config, &session_id_param, &tmux_name, &data).await;
                                    if let Some(delay_ms) = input_snapshot_delay_ms(&data) {
                                        let submitted = is_submit_input(&data);
                                        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                                        if terminal::should_snapshot_after_input(&config_clone, &session_id_param_clone, &tmux_name_clone, submitted).await {
                                            if !send_terminal_snapshot(&mut ws, &config_clone, &session_id_param_clone, &tmux_name_clone, 500).await {
                                                break;
                                            }
                                            skip_captured_output(&mut output_file_handle, &mut last_output_size).await;
                                        }
                                    }
                                }
                                TerminalMessage::Resize { cols, rows } => {
                                    let _ = terminal::resize_tmux(&state.config, &session_id_param, &tmux_name, cols, rows).await;
                                    if !history_sent {
                                        if !send_terminal_snapshot(&mut ws, &config_clone, &session_id_param_clone, &tmux_name_clone, 500).await {
                                            break;
                                        }
                                        history_sent = true;
                                    }
                                }
                                TerminalMessage::CloseTerminal => break,
                                _ => {}
                            }
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

async fn recover_tmux_session(
    config: &AppConfig,
    session_id: &str,
    auth: &AuthUser,
) -> Option<TerminalSession> {
    if !session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return None;
    }

    let tmux_name = format!("rmux_{}", session_id);
    if !terminal::check_tmux_session(config, session_id, &tmux_name).await {
        return None;
    }

    let cwd = terminal::query_tmux_cwd(config, session_id, &tmux_name, "/").await;
    let now = chrono::Utc::now().to_rfc3339();
    Some(TerminalSession {
        session_id: session_id.to_string(),
        name: "本地终端".into(),
        owner_uid: auth.uid,
        owner_user: auth.user.clone(),
        session_type: "local".into(),
        created_at: now.clone(),
        last_activity: now,
        size: TermSize { cols: 80, rows: 24 },
        is_new: false,
        tmux_session_name: tmux_name,
        output_file: config.outputs_dir.join(format!("{}.out", session_id)),
        cwd,
    })
}

fn input_snapshot_delay_ms(data: &str) -> Option<u64> {
    if data.is_empty() {
        None
    } else if is_submit_input(data) {
        Some(ENTER_SNAPSHOT_DELAY_MS)
    } else {
        Some(INPUT_SNAPSHOT_DELAY_MS)
    }
}

fn is_submit_input(data: &str) -> bool {
    data.contains('\n') || data.contains('\r')
}

async fn skip_captured_output(
    output_file_handle: &mut Option<tokio::fs::File>,
    last_output_size: &mut u64,
) {
    let Some(file) = output_file_handle.as_mut() else {
        return;
    };

    if let Ok(meta) = file.metadata().await {
        *last_output_size = meta.len();
        let _ = file.seek(SeekFrom::Start(*last_output_size)).await;
    }
}

async fn send_terminal_snapshot(
    ws: &mut WebSocket,
    config: &AppConfig,
    session_id: &str,
    tmux_name: &str,
    lines: u32,
) -> bool {
    match terminal::capture_tmux_snapshot(config, session_id, tmux_name, lines).await {
        Ok(history) => ws
            .send(Message::Text(format!("\x1b[2J\x1b[H{}", history).into()))
            .await
            .is_ok(),
        Err(e) => {
            warn!("同步终端画面失败: {}", e);
            true
        }
    }
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
            let _ = tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666));
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
    for row in rows {
        if let Ok(item) = row {
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

async fn get_theme(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
) -> Json<ApiResponse<Value>> {
    let conn = state.db.get().unwrap();
    let mut stmt = conn.prepare("SELECT theme, font_size, background_opacity, background_image, background_image_type, saved_url_image, saved_upload_image, tab_style, cursor_style, component_customization FROM theme_settings WHERE owner_uid=?1 AND is_active=1 ORDER BY id DESC LIMIT 1").unwrap();
    let result = stmt.query_row([auth.0.uid], |row| {
        Ok(serde_json::json!({
            "theme": row.get::<_, String>(0)?, "fontSize": row.get::<_, i64>(1)?, "backgroundOpacity": row.get::<_, i64>(2)?,
            "backgroundImage": row.get::<_, String>(3)?, "backgroundImageType": row.get::<_, String>(4)?, "savedUrlImage": row.get::<_, String>(5)?,
            "savedUploadImage": row.get::<_, String>(6)?, "tabStyle": row.get::<_, String>(7)?, "cursorStyle": row.get::<_, String>(8)?,
            "componentCustomization": serde_json::from_str::<Value>(&row.get::<_, String>(9)?).unwrap_or_default(),
        }))
    });
    match result {
        Ok(settings) => Json(ApiResponse::ok(settings)),
        Err(_) => Json(ApiResponse::ok(
            serde_json::to_value(ThemeSettings::default()).unwrap(),
        )),
    }
}

async fn save_theme(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
    Json(body): Json<Value>,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    let conn = state.db.get().map_err(|e| ApiError::new(e.to_string()))?;
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    conn.execute(
        "UPDATE theme_settings SET is_active=0 WHERE owner_uid=?1",
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
    let conn = state.db.get().map_err(|e| ApiError::new(e.to_string()))?;
    conn.execute(
        "DELETE FROM theme_settings WHERE owner_uid=?1",
        [auth.0.uid],
    )
    .ok();
    Ok(Json(ApiResponse::ok(
        serde_json::to_value(ThemeSettings::default()).unwrap(),
    )))
}

async fn upload_background(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    while let Some(field) = multipart.next_field().await.unwrap_or(None) {
        let _name = field.file_name().unwrap_or("bg.png").to_string();
        let data = field
            .bytes()
            .await
            .map_err(|e| ApiError::new(e.to_string()))?;
        let filename = format!("{}_{}.png", auth.0.uid, chrono::Utc::now().timestamp());
        let path = state.config.backgrounds_dir.join(&filename);
        tokio::fs::write(&path, &data)
            .await
            .map_err(|e| ApiError::new(e.to_string()))?;
        return Ok(Json(ApiResponse::ok(
            serde_json::json!({ "filename": filename }),
        )));
    }
    Err(ApiError::new("No file"))
}

async fn get_background_file(
    State(state): State<AppState>,
    Path(filename): Path<String>,
) -> Result<Response, StatusCode> {
    let path = state.config.backgrounds_dir.join(&filename);
    if let Ok(data) = tokio::fs::read(&path).await {
        Ok(([(header::CONTENT_TYPE, "image/png")], data).into_response())
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn save_background_url(
    State(state): State<AppState>,
    auth: AuthUserExtractor,
    Json(body): Json<Value>,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    let url = body
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or(ApiError::new("No url"))?;
    let data = reqwest::get(url)
        .await
        .map_err(|e| ApiError::new(e.to_string()))?
        .bytes()
        .await
        .map_err(|e| ApiError::new(e.to_string()))?;
    let filename = format!("{}_{}_url.png", auth.0.uid, chrono::Utc::now().timestamp());
    tokio::fs::write(state.config.backgrounds_dir.join(&filename), &data)
        .await
        .map_err(|e| ApiError::new(e.to_string()))?;
    Ok(Json(ApiResponse::ok(
        serde_json::json!({ "filename": filename }),
    )))
}

async fn get_system_info() -> Json<ApiResponse<Value>> {
    let hostname =
        std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_else(|_| "unknown".into());
    Json(ApiResponse::ok(
        serde_json::json!({ "hostname": hostname.trim(), "os": "Linux", "arch": std::env::consts::ARCH }),
    ))
}

async fn purge_data(
    State(state): State<AppState>,
    _auth: AuthUserExtractor,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    // 危险操作：清空所有用户数据 (logs, outputs, db 等)
    // 注意：不删除正在使用的 tmux socket
    let _ = std::fs::remove_dir_all(&state.config.outputs_dir);
    let _ = std::fs::remove_dir_all(&state.config.logs_dir);
    let _ = std::fs::remove_dir_all(&state.config.backgrounds_dir);
    let _ = state.config.ensure_dirs();
    Ok(Json(ApiResponse::ok(serde_json::json!({}))))
}
