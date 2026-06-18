use std::env;
use std::path::PathBuf;

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct AppConfig {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub jwt_secret_file: PathBuf,
    pub auth_file: PathBuf,
    pub skip_auth_file: PathBuf,
    pub session_file: PathBuf,
    pub backgrounds_dir: PathBuf,
    pub outputs_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub sessions_dir: PathBuf,
    pub ui_dir: PathBuf,
    pub socket_path: PathBuf,
    pub clipboard_dir: PathBuf,
    pub log_file: PathBuf,
    pub jwt_secret: String,
    pub host: String,
    pub port: u16,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let app_dir = env::var("RMUX_APPDIR").unwrap_or_else(|_| ".".to_string());
        let data_dir = env::var("RMUX_DATA_DIR").unwrap_or_else(|_| "var".to_string());
        let data_dir = PathBuf::from(&data_dir);

        let jwt_secret = env::var("JWT_SECRET").unwrap_or_else(|_| String::new());

        AppConfig {
            jwt_secret,
            host: env::var("RMUX_HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("RMUX_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(18732),
            data_dir: data_dir.clone(),
            db_path: data_dir.join("rmux.db"),
            jwt_secret_file: data_dir.join("rmux_jwt_secret_v2"),
            auth_file: data_dir.join("rmux_auth.json"),
            skip_auth_file: data_dir.join(".skip_auth"),
            session_file: data_dir.join("rmux_sessions.json"),
            backgrounds_dir: data_dir.join("backgrounds"),
            outputs_dir: data_dir.join("outputs"),
            logs_dir: data_dir.join("logs"),
            sessions_dir: data_dir.join("sessions"),
            ui_dir: env::var("RMUX_UI_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(&app_dir).join("ui")),
            socket_path: PathBuf::from("/tmp/rmux_socks"),
            clipboard_dir: PathBuf::from("/tmp/rmux_clipboard"),
            log_file: data_dir.join("rmux.log"),
        }
    }

    /// 获取 tmux 二进制路径
    pub fn tmux_path(&self) -> PathBuf {
        if let Ok(path) = env::var("RMUX_TMUX") {
            return PathBuf::from(path);
        }
        PathBuf::from("tmux")
    }

    /// 获取 tmux 库目录
    pub fn tmux_lib_dir(&self) -> Option<PathBuf> {
        env::var("RMUX_TMUX_LIB_DIR").ok().map(PathBuf::from)
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.backgrounds_dir)?;

        std::fs::create_dir_all(&self.outputs_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&self.outputs_dir, std::fs::Permissions::from_mode(0o777));
        }

        std::fs::create_dir_all(&self.logs_dir)?;
        std::fs::create_dir_all(&self.sessions_dir)?;

        std::fs::create_dir_all(&self.socket_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&self.socket_path, std::fs::Permissions::from_mode(0o777));
        }

        std::fs::create_dir_all(&self.clipboard_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                &self.clipboard_dir,
                std::fs::Permissions::from_mode(0o777),
            );
        }
        Ok(())
    }
}
