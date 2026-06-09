use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::auth::home_for_user;
use crate::config::AppConfig;
use crate::models::*;

// ─── Session Registry ───────────────────────────────────────────────────

pub type SessionRegistry = Arc<RwLock<HashMap<String, TerminalSession>>>;

pub fn new_session_registry() -> SessionRegistry {
    Arc::new(RwLock::new(HashMap::new()))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub async fn query_tmux_cwd(
    config: &AppConfig,
    session_id: &str,
    session_name: &str,
    fallback: &str,
) -> String {
    let socket = config.socket_path.join(format!("tmux_{}.sock", session_id));
    let mut cmd = tokio::process::Command::new(config.tmux_path());
    if let Some(lib) = config.tmux_lib_dir() {
        cmd.env("LD_LIBRARY_PATH", lib);
    }
    cmd.args([
        "-u",
        "-S",
        &socket.display().to_string(),
        "display-message",
        "-p",
        "-t",
        session_name,
        "#{pane_current_path}",
    ]);

    match cmd.output().await {
        Ok(output) if output.status.success() => {
            let cwd = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if cwd.is_empty() {
                fallback.to_string()
            } else {
                cwd
            }
        }
        _ => fallback.to_string(),
    }
}

// ─── Tmux Manager ───────────────────────────────────────────────────────

pub async fn create_local_session(
    config: &AppConfig,
    session_id: &str,
    _login_user: &crate::models::AuthUser,
    cols: u32,
    rows: u32,
) -> Result<TerminalSession, String> {
    let tmux_name = format!("rmux_{}", session_id);
    let output_file = config.outputs_dir.join(format!("{}.out", session_id));
    let tmux_bin = config.tmux_path();

    std::fs::create_dir_all(&config.socket_path)
        .map_err(|e| format!("创建 tmux socket 目录失败: {}", e))?;
    std::fs::create_dir_all(&config.outputs_dir).map_err(|e| format!("创建输出目录失败: {}", e))?;
    let _ = std::fs::set_permissions(&config.socket_path, std::fs::Permissions::from_mode(0o777));
    let _ = std::fs::set_permissions(&config.outputs_dir, std::fs::Permissions::from_mode(0o777));

    // 尝试寻找可用的 shell
    let shell = if std::path::Path::new("/bin/bash").exists() {
        "/bin/bash"
    } else if std::path::Path::new("/usr/bin/bash").exists() {
        "/usr/bin/bash"
    } else {
        "/bin/sh"
    };

    info!(
        "准备创建本地 tmux 会话: {}, 窗口: {}x{}, 目标用户: {}",
        tmux_name, cols, rows, _login_user.user
    );
    let target_home = home_for_user(&_login_user.user);

    // 寻找用户切换工具
    let runuser_bin = ["/usr/sbin/runuser", "/usr/bin/runuser", "runuser"]
        .iter()
        .find(|p| std::path::Path::new(p).exists() || !p.starts_with('/'))
        .unwrap_or(&"runuser");

    let su_bin = ["/bin/su", "/usr/bin/su", "su"]
        .iter()
        .find(|p| std::path::Path::new(p).exists() || !p.starts_with('/'))
        .unwrap_or(&"su");

    // 构造唯一的 socket 文件路径
    let session_socket = config.socket_path.join(format!("tmux_{}.sock", session_id));
    let socket_str = session_socket.display().to_string();

    // 构造内部 tmux 命令
    let mut tmux_cmd_str = tmux_bin.display().to_string();
    if let Some(lib) = config.tmux_lib_dir() {
        tmux_cmd_str = format!("export LD_LIBRARY_PATH={}:${{LD_LIBRARY_PATH:-}}; export LANG=C.UTF-8; export LC_ALL=C.UTF-8; {}", lib.display(), tmux_cmd_str);
    } else {
        tmux_cmd_str = format!(
            "export LANG=C.UTF-8; export LC_ALL=C.UTF-8; {}",
            tmux_cmd_str
        );
    }

    let tmux_args = [
        "-u", // 强制 UTF-8
        "-S",
        &socket_str,
        "new-session",
        "-d",
        "-s",
        &tmux_name,
        "-x",
        &cols.to_string(),
        "-y",
        &rows.to_string(),
        shell,
    ]
    .join(" ");

    let current_uid = unsafe { libc::getuid() } as i64;
    let launch_directly = _login_user.uid == current_uid;
    let tmux_command = format!("{} {}", tmux_cmd_str, tmux_args);
    let full_command = if let Some(home) = &target_home {
        format!("cd {} && {}", shell_quote(home), tmux_command)
    } else {
        tmux_command
    };

    let mut cmd;
    if launch_directly {
        cmd = tokio::process::Command::new(&tmux_bin);
        if let Some(home) = &target_home {
            cmd.current_dir(home);
        }
        cmd.args([
            "-u",
            "-S",
            &socket_str,
            "new-session",
            "-d",
            "-s",
            &tmux_name,
            "-x",
            &cols.to_string(),
            "-y",
            &rows.to_string(),
            shell,
        ]);
    } else if std::path::Path::new(runuser_bin).exists() {
        cmd = tokio::process::Command::new(runuser_bin);
        cmd.args(["-l", &_login_user.user, "-c", &full_command]);
    } else {
        cmd = tokio::process::Command::new(su_bin);
        cmd.args(["-", &_login_user.user, "-c", &full_command]);
    }

    if let Some(lib) = config.tmux_lib_dir() {
        cmd.env("LD_LIBRARY_PATH", lib);
    }
    cmd.env("SHELL", shell);

    let result = cmd.output().await;

    match result {
        Ok(output) if output.status.success() => {
            info!("✅ 成功发起 tmux 创建请求 (用户: {})", _login_user.user);

            // 稍等片刻检查
            tokio::time::sleep(tokio::time::Duration::from_millis(600)).await;

            if !check_tmux_session(config, session_id, &tmux_name).await {
                let err_msg = String::from_utf8_lossy(&output.stderr);
                error!("❌ 终端会话创建后未发现或已退出. 详情: {}", err_msg);
                return Err("终端创建失败，请检查系统账户权限".into());
            }

            // 启用输出捕获
            let mut pipe_cmd;
            let mut pipe_tmux_str = tmux_bin.display().to_string();
            if let Some(lib) = config.tmux_lib_dir() {
                pipe_tmux_str = format!(
                    "export LD_LIBRARY_PATH={}:${{LD_LIBRARY_PATH:-}}; export LANG=C.UTF-8; {}",
                    lib.display(),
                    pipe_tmux_str
                );
            } else {
                pipe_tmux_str = format!("export LANG=C.UTF-8; {}", pipe_tmux_str);
            }

            let pipe_args = format!(
                "{} -u -S {} pipe-pane -t {} -o 'cat >> {}'",
                pipe_tmux_str,
                socket_str,
                tmux_name,
                output_file.display()
            );

            if launch_directly {
                pipe_cmd = tokio::process::Command::new(&tmux_bin);
                pipe_cmd.args([
                    "-u",
                    "-S",
                    &socket_str,
                    "pipe-pane",
                    "-t",
                    &tmux_name,
                    "-o",
                    &format!("cat >> {}", output_file.display()),
                ]);
            } else if std::path::Path::new(runuser_bin).exists() {
                pipe_cmd = tokio::process::Command::new(runuser_bin);
                pipe_cmd.args(["-l", &_login_user.user, "-c", &pipe_args]);
            } else {
                pipe_cmd = tokio::process::Command::new(su_bin);
                pipe_cmd.args(["-", &_login_user.user, "-c", &pipe_args]);
            }
            pipe_cmd.output().await.ok();
            info!("📝 已启用 pipe-pane 输出捕获: {}", output_file.display());

            let now = chrono::Utc::now().to_rfc3339();
            Ok(TerminalSession {
                session_id: session_id.to_string(),
                name: "本地终端".into(),
                owner_uid: _login_user.uid,
                owner_user: _login_user.user.clone(),
                session_type: "local".into(),
                created_at: now.clone(),
                last_activity: now,
                size: TermSize { cols, rows },
                is_new: true,
                tmux_session_name: tmux_name,
                output_file,
                cwd: target_home.clone().unwrap_or_else(|| "/".into()),
            })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            error!(
                "创建 tmux 会话失败. ExitCode: {:?}, Stderr: {}, Stdout: {}",
                output.status.code(),
                stderr,
                stdout
            );
            Err(format!("创建 tmux 会话失败: {}", stderr))
        }
        Err(e) => {
            error!(
                "tmux 命令执行失败 (二进制路径: {}): {}",
                tmux_bin.display(),
                e
            );
            Err(format!("tmux 命令执行失败: {}", e))
        }
    }
}

pub async fn write_to_tmux(
    config: &AppConfig,
    session_id: &str,
    session_name: &str,
    data: &str,
) -> Result<(), String> {
    let socket = config.socket_path.join(format!("tmux_{}.sock", session_id));
    let mut cmd = tokio::process::Command::new(config.tmux_path());
    if let Some(lib) = config.tmux_lib_dir() {
        cmd.env("LD_LIBRARY_PATH", lib);
    }
    cmd.args([
        "-u",
        "-S",
        &socket.display().to_string(),
        "send-keys",
        "-t",
        session_name,
        "-l",
        data,
    ]);
    let result = cmd.output().await;
    match result {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(String::from_utf8_lossy(&output.stderr).to_string()),
        Err(e) => Err(format!("tmux error: {}", e)),
    }
}

pub async fn paste_to_tmux(
    config: &AppConfig,
    session_id: &str,
    session_name: &str,
    data: &str,
) -> Result<(), String> {
    if data.is_empty() {
        return Ok(());
    }

    let socket = config.socket_path.join(format!("tmux_{}.sock", session_id));
    tokio::fs::create_dir_all(&config.clipboard_dir)
        .await
        .map_err(|e| format!("创建粘贴临时目录失败: {}", e))?;

    let buffer_name = format!("rmux_paste_{}", uuid::Uuid::new_v4().simple());
    let paste_file = config.clipboard_dir.join(format!("{}.txt", buffer_name));
    tokio::fs::write(&paste_file, data)
        .await
        .map_err(|e| format!("写入粘贴临时文件失败: {}", e))?;

    let paste_file_str = paste_file.display().to_string();
    let socket_str = socket.display().to_string();

    let mut load_cmd = tokio::process::Command::new(config.tmux_path());
    if let Some(lib) = config.tmux_lib_dir() {
        load_cmd.env("LD_LIBRARY_PATH", lib);
    }
    load_cmd.args([
        "-u",
        "-S",
        &socket_str,
        "load-buffer",
        "-b",
        &buffer_name,
        &paste_file_str,
    ]);

    let load_output = load_cmd
        .output()
        .await
        .map_err(|e| format!("tmux load-buffer 执行失败: {}", e))?;
    let _ = tokio::fs::remove_file(&paste_file).await;

    if !load_output.status.success() {
        return Err(String::from_utf8_lossy(&load_output.stderr).to_string());
    }

    let mut paste_cmd = tokio::process::Command::new(config.tmux_path());
    if let Some(lib) = config.tmux_lib_dir() {
        paste_cmd.env("LD_LIBRARY_PATH", lib);
    }
    paste_cmd.args([
        "-u",
        "-S",
        &socket_str,
        "paste-buffer",
        "-d",
        "-p",
        "-b",
        &buffer_name,
        "-t",
        session_name,
    ]);

    let paste_output = paste_cmd
        .output()
        .await
        .map_err(|e| format!("tmux paste-buffer 执行失败: {}", e))?;
    if paste_output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&paste_output.stderr).to_string())
    }
}

pub async fn resize_tmux(
    config: &AppConfig,
    session_id: &str,
    session_name: &str,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let socket = config.socket_path.join(format!("tmux_{}.sock", session_id));
    let mut cmd = tokio::process::Command::new(config.tmux_path());
    if let Some(lib) = config.tmux_lib_dir() {
        cmd.env("LD_LIBRARY_PATH", lib);
    }
    cmd.args([
        "-u",
        "-S",
        &socket.display().to_string(),
        "resize-window",
        "-t",
        session_name,
        "-x",
        &cols.to_string(),
        "-y",
        &rows.to_string(),
    ]);
    let result = cmd.output().await;
    match result {
        Ok(output) if output.status.success() => Ok(()),
        Ok(_) => Ok(()),
        Err(e) => Err(format!("resize error: {}", e)),
    }
}

pub async fn kill_tmux_session(
    config: &AppConfig,
    session_id: &str,
    session_name: &str,
    kill: bool,
) -> Result<(), String> {
    if kill {
        let socket = config.socket_path.join(format!("tmux_{}.sock", session_id));
        let mut cmd = tokio::process::Command::new(config.tmux_path());
        if let Some(lib) = config.tmux_lib_dir() {
            cmd.env("LD_LIBRARY_PATH", lib);
        }
        cmd.args([
            "-u",
            "-S",
            &socket.display().to_string(),
            "kill-session",
            "-t",
            session_name,
        ]);
        let _ = cmd.output().await;

        // 清理 socket 文件
        let _ = tokio::fs::remove_file(socket).await;
        info!("已杀死会话并清理 socket: {}", session_name);
    }
    Ok(())
}

pub async fn capture_tmux_history(
    config: &AppConfig,
    session_id: &str,
    session_name: &str,
    lines: u32,
) -> Result<String, String> {
    match capture_tmux_history_inner(config, session_id, session_name, lines, true).await {
        Ok(history) => Ok(history),
        Err(_) => capture_tmux_history_inner(config, session_id, session_name, lines, false).await,
    }
}

async fn capture_tmux_history_inner(
    config: &AppConfig,
    session_id: &str,
    session_name: &str,
    lines: u32,
    alternate_screen: bool,
) -> Result<String, String> {
    let socket = config.socket_path.join(format!("tmux_{}.sock", session_id));
    let mut cmd = tokio::process::Command::new(config.tmux_path());
    if let Some(lib) = config.tmux_lib_dir() {
        cmd.env("LD_LIBRARY_PATH", lib);
    }
    cmd.args([
        "-u",
        "-S",
        &socket.display().to_string(),
        "capture-pane",
        "-e", // 包含转义序列（颜色等）
    ]);
    if alternate_screen {
        cmd.arg("-a");
    }
    cmd.args(["-t", session_name, "-S", &format!("-{}", lines), "-p"]);
    let result = cmd.output().await;
    match result {
        Ok(output) if output.status.success() => {
            let history = String::from_utf8_lossy(&output.stdout).to_string();
            // 确保换行符是 \r\n，防止阶梯效应
            Ok(compact_captured_history(&history).replace("\n", "\r\n"))
        }
        Ok(output) => Err(String::from_utf8_lossy(&output.stderr).to_string()),
        Err(e) => Err(format!("capture error: {}", e)),
    }
}

fn compact_captured_history(history: &str) -> String {
    let mut out = Vec::new();
    let mut blank_run = 0usize;

    for line in history.lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 2 {
                out.push(line);
            }
            continue;
        }

        blank_run = 0;
        out.push(line);
    }

    while out
        .last()
        .map(|line| line.trim().is_empty())
        .unwrap_or(false)
    {
        out.pop();
    }

    out.join("\n")
}

pub async fn check_tmux_session(config: &AppConfig, session_id: &str, session_name: &str) -> bool {
    let socket = config.socket_path.join(format!("tmux_{}.sock", session_id));
    let mut cmd = tokio::process::Command::new(config.tmux_path());
    if let Some(lib) = config.tmux_lib_dir() {
        cmd.env("LD_LIBRARY_PATH", lib);
    }
    cmd.args([
        "-u",
        "-S",
        &socket.display().to_string(),
        "has-session",
        "-t",
        session_name,
    ])
    .output()
    .await
    .map(|o| o.status.success())
    .unwrap_or(false)
}
