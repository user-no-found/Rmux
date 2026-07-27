use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

// ─── Auth ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AuthUser {
    pub uid: i64,
    pub user: String,
    pub is_admin: bool,
    pub groups: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AppPasswordStatus {
    pub status: String, // "setup", "login", "public"
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SetPasswordRequest {
    pub password: Option<String>, // None means skip/remove
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub user: AuthUser,
}

// ─── Sessions ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub name: String,
    pub owner_uid: i64,
    pub owner_user: String,
    #[serde(rename = "type")]
    pub session_type: String, // "local"
    pub status: String, // "active"
    pub created_at: String,
    pub last_activity: String,
    pub size: TermSize,
    pub cwd: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TermSize {
    pub cols: u32,
    pub rows: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TerminalSession {
    pub session_id: String,
    pub name: String,
    pub owner_uid: i64,
    pub owner_user: String,
    pub session_type: String,
    pub created_at: String,
    pub last_activity: String,
    pub size: TermSize,
    pub tmux_session_name: String,
    pub cwd: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(rename = "type")]
    pub session_type: String,
    pub cols: u32,
    pub rows: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateSessionNameRequest {
    pub name: String,
}

// ─── Clipboard ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClipboardItem {
    pub id: String,
    pub kind: String, // "text" | "image"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "contentType")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RecordClipboardRequest {
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default, rename = "contentType")]
    pub content_type: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
}

// ─── Theme ──────────────────────────────────────────────────────────────

/// camelCase 与 `get_theme` 命中分支手写的 JSON 保持一致 —— 前端按 camelCase
/// 取值，两条分支若用不同命名，新用户拿到的服务端默认值会全部落空。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSettings {
    pub theme: String,
    pub font_size: u32,
    pub background_opacity: u32,
    pub background_image: String,
    pub background_image_type: String,
    pub saved_url_image: String,
    pub saved_upload_image: String,
    pub tab_style: String,
    pub cursor_style: String,
    pub component_customization: serde_json::Value,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            theme: "onedark".into(),
            font_size: 14,
            background_opacity: 0,
            background_image: String::new(),
            background_image_type: "none".into(),
            saved_url_image: String::new(),
            saved_upload_image: String::new(),
            tab_style: "modern".into(),
            cursor_style: "block".into(),
            component_customization: serde_json::json!({
                "interface": {"mode": "default", "color": "#282c34"},
                "button": {"mode": "default", "color": "#0e639c", "opacity": 100},
                "textSelection": {"mode": "default", "color": "#0ea5e9", "opacity": 40},
            }),
        }
    }
}

// ─── API Response ───────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    pub message: String,
    pub data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            message: "ok".into(),
            data: Some(data),
        }
    }
}

fn default_error_status() -> u16 {
    StatusCode::BAD_REQUEST.as_u16()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub success: bool,
    pub message: String,
    /// HTTP 状态码。不进入响应体，只决定响应头。
    #[serde(skip, default = "default_error_status")]
    pub status: u16,
}

impl ApiError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self::with_status(StatusCode::BAD_REQUEST, msg)
    }

    pub fn with_status(status: StatusCode, msg: impl Into<String>) -> Self {
        Self {
            success: false,
            message: msg.into(),
            status: status.as_u16(),
        }
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::with_status(StatusCode::FORBIDDEN, msg)
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::with_status(StatusCode::NOT_FOUND, msg)
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::with_status(StatusCode::INTERNAL_SERVER_ERROR, msg)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::BAD_REQUEST);
        (status, Json(self)).into_response()
    }
}

// ─── WebSocket Messages ─────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TerminalMessage {
    #[serde(rename = "input")]
    Input { data: String },
    #[serde(rename = "paste")]
    Paste { data: String },
    #[serde(rename = "resize")]
    Resize { cols: u32, rows: u32 },
    #[serde(rename = "resume_session")]
    ResumeSession { session_id: String },
    #[serde(rename = "close_terminal")]
    CloseTerminal,
    #[serde(rename = "keepalive")]
    KeepAlive,
}
