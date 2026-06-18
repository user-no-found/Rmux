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
use tower_http::cors::{Any, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::services::{
    fs::ServeFileSystemResponseBody,
    ServeDir, ServeFile,
};
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

    // Create session registry
    let sessions = terminal::new_session_registry();

    // Build app state
    let state = api::AppState {
        config: config.clone(),
        db: pool,
        sessions,
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

    // Build router
    let app = api::build_router(state.clone())
        .layer(Extension(config.clone()))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .fallback_service(static_files);

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
