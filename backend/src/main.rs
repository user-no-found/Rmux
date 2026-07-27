mod api;
mod auth;
mod config;
mod db;
mod models;
mod terminal;

use axum::{
    http::{
        header::{CACHE_CONTROL, CONTENT_TYPE},
        HeaderValue,
    },
    Extension,
};
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::services::{fs::ServeFileSystemResponseBody, ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // Load config
    let cfg = config::AppConfig::from_env();
    cfg.ensure_dirs()
        .expect("Failed to create data directories");

    tracing::info!(
        "身份信息: UID={}, GID={}",
        unsafe { libc::getuid() },
        unsafe { libc::getgid() }
    );

    // Initialize JWT secret
    let jwt_secret = init_jwt_secret(&cfg);
    let mut cfg = cfg;
    cfg.jwt_secret = jwt_secret;
    let config = Arc::new(cfg);

    // Initialize Database
    let pool = db::create_pool(&config);

    // Create session registry，并把上次运行留下的、tmux 里仍然活着的会话捞回来。
    // 不恢复的话：重启后 tmux server 还在跑，会话却既列不出来也连不上，
    // 而且永远不会被回收。
    let sessions = terminal::new_session_registry();
    let restored = terminal::restore_sessions(&config).await;
    if !restored.is_empty() {
        tracing::info!("恢复了 {} 个仍在运行的终端会话", restored.len());
    }
    *sessions.write().await = restored;
    // 立即回写一次，把已经消失的条目从文件里剔掉。
    terminal::persist_sessions(&config, &sessions).await;

    // Build app state
    let state = api::AppState {
        config: config.clone(),
        db: pool,
        sessions,
        attaches: terminal::new_attach_registry(),
    };

    let index_file = config.ui_dir.join("index.html");
    let static_files = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
            CACHE_CONTROL,
            |response: &axum::http::Response<ServeFileSystemResponseBody>| {
                let content_type = response
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default();
                if content_type.starts_with("text/html") {
                    Some(HeaderValue::from_static("no-cache"))
                } else {
                    None
                }
            },
        ))
        .service(ServeDir::new(&config.ui_dir).fallback(ServeFile::new(index_file)));

    // fnOS 桌面/应用商店从 /app/rmux/images/ 读取图标，而图标实际位于
    // app/ui/images/（与 app/www/ 同级），不会被 ui_dir 的 SPA fallback 命中。
    // 单独把它 serve 出来，否则 fnOS 拿到的是 index.html，前端会显示首字母占位图。
    let images_dir = config
        .ui_dir
        .parent()
        .map(|p| p.join("ui").join("images"))
        .unwrap_or_else(|| std::path::PathBuf::from("ui/images"));
    let images_service = ServeDir::new(&images_dir);

    // Build router
    //
    // 静态服务要先挂上再 .layer()：Router::layer 只包裹此前已注册的路由，
    // 原先的顺序让所有静态资源既拿不到 Extension 也不产生 tracing span。
    //
    // 这里刻意不再挂 CorsLayer。原先是 allow_origin(Any) + allow_methods(Any)
    // + allow_headers(Any)：前端与 API 本来就同源（fnOS 反代到 /app/rmux，
    // 开发态走 vite proxy），根本不需要 CORS；而放开之后，一旦处于免密模式，
    // 用户浏览到的任意网页都能跨源调用本机 API 建终端、读输出 —— 等于一个
    // 挂在浏览器上的远程执行入口。
    let app = api::build_router(state.clone())
        .nest_service("/app/rmux/images", images_service.clone())
        .nest_service("/images", images_service)
        .nest_service("/app/rmux", static_files.clone())
        .fallback_service(static_files)
        .layer(Extension(config.clone()))
        .layer(tower_http::trace::TraceLayer::new_for_http());

    // Start server
    let addr = format!("{}:{}", config.host, config.port);
    tracing::info!("Rmux 服务启动: http://{}", addr);
    tracing::info!("📁 数据目录: {}", config.data_dir.display());
    tracing::info!("🗄️  数据库: {}", config.db_path.display());

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

fn init_jwt_secret(cfg: &config::AppConfig) -> String {
    // Try loading from env
    if !cfg.jwt_secret.is_empty() {
        tracing::info!("从环境变量加载JWT密钥");
        return cfg.jwt_secret.clone();
    }

    // Try loading from file
    if cfg.jwt_secret_file.exists() {
        if let Ok(content) = std::fs::read_to_string(&cfg.jwt_secret_file) {
            let secret = content.trim().to_string();
            if !secret.is_empty() {
                tracing::info!("从文件加载JWT密钥");
                return secret;
            }
        }
    }

    // Generate new
    let secret = uuid::Uuid::new_v4().to_string() + &uuid::Uuid::new_v4().to_string();
    if let Some(parent) = cfg.jwt_secret_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&cfg.jwt_secret_file, &secret);
    tracing::info!("生成并保存新的JWT密钥");
    secret
}
