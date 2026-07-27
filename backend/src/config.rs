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
        let data_dir = env::var("TRIM_PKGVAR").unwrap_or_else(|_| "/tmp/rmux".to_string());
        let data_dir = PathBuf::from(&data_dir);

        let app_dest = env::var("TRIM_APPDEST").unwrap_or_else(|_| ".".to_string());

        let jwt_secret = env::var("JWT_SECRET").unwrap_or_else(|_| String::new());

        AppConfig {
            jwt_secret,
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(8080),
            data_dir: data_dir.clone(),
            db_path: data_dir.join("rmux.db"),
            jwt_secret_file: data_dir.join("rmux_jwt_secret_v2"),
            auth_file: data_dir.join("rmux_auth.json"),
            skip_auth_file: data_dir.join(".skip_auth"),
            session_file: data_dir.join("rmux_sessions.json"),
            backgrounds_dir: data_dir.join("backgrounds"),
            logs_dir: data_dir.join("logs"),
            sessions_dir: data_dir.join("sessions"),
            ui_dir: PathBuf::from(&app_dest).join("www"),
            socket_path: PathBuf::from("/tmp/rmux_socks"),
            clipboard_dir: PathBuf::from("/tmp/rmux_clipboard"),
            log_file: data_dir.join("rmux.log"),
        }
    }

    /// 获取内置 tmux 二进制路径
    pub fn tmux_path(&self) -> PathBuf {
        if let Some(app_root) = self.app_root_dir() {
            let p = app_root.join("server/bin/tmux");
            if p.exists() {
                return p;
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent().and_then(|p| p.parent()) {
                let p = exe_dir.join("server/bin/tmux");
                if p.exists() {
                    return p;
                }
            }
        }
        PathBuf::from("tmux")
    }

    /// 获取内置 tmux 的库目录
    pub fn tmux_lib_dir(&self) -> Option<PathBuf> {
        if let Some(root) = self.app_root_dir() {
            let p = root.join("server/lib");
            if p.join("libutempter.so.0").exists() {
                return Some(p);
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent().and_then(|p| p.parent()) {
                let p = exe_dir.join("server/lib");
                if p.join("libutempter.so.0").exists() {
                    return Some(p);
                }
            }
        }
        None
    }

    pub fn app_root_dir(&self) -> Option<PathBuf> {
        let app_dest = env::var("TRIM_APPDEST").ok()?;
        let p = PathBuf::from(app_dest);
        if p.is_absolute() {
            Some(p)
        } else {
            None
        }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.backgrounds_dir)?;
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
