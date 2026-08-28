use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

use crate::auth::home_for_user;
use crate::config::AppConfig;
use crate::models::*;

// ─── Session Registry ───────────────────────────────────────────────────

pub type SessionRegistry = Arc<RwLock<HashMap<String, TerminalSession>>>;

pub fn new_session_registry() -> SessionRegistry {
    Arc::new(RwLock::new(HashMap::new()))
}

// ─── 会话注册表持久化 ───────────────────────────────────────────────────

/// tmux server 本身跨进程存活，但 `session_id → tmux 会话名/归属/尺寸` 的映射
/// 原先只在内存里，服务一重启就全丢：tmux 里的 shell 还活着，却既列不出来、
/// 也连不上（只会收到 SESSION_NOT_FOUND），而且再没有任何东西会去回收它。
/// 把映射落盘，重启后校验着捞回来，"会话持久化"才对重启也成立。
pub async fn persist_sessions(config: &AppConfig, registry: &SessionRegistry) {
    let snapshot: Vec<TerminalSession> = registry.read().await.values().cloned().collect();

    let payload = match serde_json::to_vec_pretty(&snapshot) {
        Ok(v) => v,
        Err(e) => {
            error!("序列化会话列表失败: {}", e);
            return;
        }
    };

    // 先写临时文件再 rename：进程随时可能被 FPK 脚本 SIGKILL，
    // 直接覆写会留下半截 JSON，下次启动就整份恢复不了。
    let tmp = config.session_file.with_extension("json.tmp");
    if let Err(e) = tokio::fs::write(&tmp, &payload).await {
        error!("写入会话列表失败: {}", e);
        return;
    }
    if let Err(e) = tokio::fs::rename(&tmp, &config.session_file).await {
        error!("提交会话列表失败: {}", e);
        let _ = tokio::fs::remove_file(&tmp).await;
    }
}

/// 启动时恢复。逐条用 `has-session` 校验：tmux server 已经不在的条目直接丢弃，
/// 顺手清掉它遗留的 socket 文件，避免 /tmp 里堆死 socket。
pub async fn restore_sessions(config: &AppConfig) -> HashMap<String, TerminalSession> {
    let mut alive: HashMap<String, TerminalSession> = HashMap::new();

    let Ok(raw) = std::fs::read(&config.session_file) else {
        return alive;
    };
    let saved: Vec<TerminalSession> = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => {
            warn!("会话列表解析失败，按空列表启动: {}", e);
            return alive;
        }
    };

    for session in saved {
        if check_tmux_session(config, &session.session_id, &session.tmux_session_name).await {
            info!(
                "恢复会话 {} ({}, 用户 {})",
                session.name, session.session_id, session.owner_user
            );
            alive.insert(session.session_id.clone(), session);
        } else {
            let socket = config
                .socket_path
                .join(format!("tmux_{}.sock", session.session_id));
            let _ = std::fs::remove_file(&socket);
            info!("丢弃已消失的会话记录: {}", session.session_id);
        }
    }

    alive
}

// ─── Attach 代号 ────────────────────────────────────────────────────────

/// 一个 tmux 会话同时只允许一个客户端（attach 带 `-d`，后来者顶掉先来者）。
/// 连接断开会自动重连，所以"被顶掉"必须和"网络断了"区分开：否则两个浏览器
/// 标签开同一个会话时会互相顶、无限重连，谁都用不了。
///
/// 每次 attach 领一个递增代号；后来者一登记，先来者的订阅就会触发，
/// 先来者据此告知前端"已被接管"并退场，前端便不再重连。
pub type AttachRegistry = Arc<RwLock<HashMap<String, tokio::sync::watch::Sender<u64>>>>;

pub fn new_attach_registry() -> AttachRegistry {
    Arc::new(RwLock::new(HashMap::new()))
}

pub async fn claim_attach(
    registry: &AttachRegistry,
    session_id: &str,
) -> tokio::sync::watch::Receiver<u64> {
    let mut map = registry.write().await;
    let tx = map
        .entry(session_id.to_string())
        .or_insert_with(|| tokio::sync::watch::channel(0).0);
    // 用 send_modify：它不要求当前存在接收端。原先是 `let _ = tx.send(next)`，
    // 而 channel(0) 的接收端建完就被丢弃了，第一次 claim 的 send 必然返回 Err
    // 并被吞掉 —— 代号压根没写进去，后续 claim 重新读 borrow 才碰巧自洽。
    tx.send_modify(|ticket| *ticket += 1);
    // 先递增再 subscribe：拿到的接收端基线就是自己的代号，
    // 只有更后面的 attach 才会让它触发。
    tx.subscribe()
}

pub async fn release_attach(registry: &AttachRegistry, session_id: &str) {
    registry.write().await.remove(session_id);
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

// ─── tmux 配置 ──────────────────────────────────────────────────────────

/// 浏览器端通过真 PTY 直连 tmux 客户端，因此 tmux 必须对按键完全透明。
/// 最关键的是关掉前缀键：输入不再走 `send-keys -l` 字面量注入，
/// 若保留默认 C-b，readline 的 backward-char 和大量 CLI 快捷键都会被 tmux 截走。
const TMUX_CONF: &str = r#"# 由 Rmux 自动生成，请勿手工编辑。
set -g prefix None
set -g prefix2 None
set -g status off
set -sg escape-time 0
set -g history-limit 10000
set -g mouse off
set -g default-terminal "tmux-256color"
set -as terminal-features ",xterm-256color:RGB"
set -g window-size latest
set -g destroy-unattached off
set -g set-titles off
setw -g alternate-screen on
setw -g automatic-rename off
"#;

/// 配置写在数据目录下，`new-session` 时用 `-f` 传入。每个会话有独立 socket，
/// 也就是独立的 tmux server，`-f` 正好在建 server 时生效。
pub fn ensure_tmux_conf(config: &AppConfig) -> Result<PathBuf, String> {
    let path = config.data_dir.join("rmux.tmux.conf");
    let stale = std::fs::read_to_string(&path)
        .map(|existing| existing != TMUX_CONF)
        .unwrap_or(true);

    if stale {
        std::fs::write(&path, TMUX_CONF).map_err(|e| format!("写入 tmux 配置失败: {}", e))?;
        // tmux server 可能以目标用户身份启动，配置必须对其可读。
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
    }
    Ok(path)
}

// ─── 进程启动方式 ───────────────────────────────────────────────────────

/// 与服务同 uid 时直接执行，否则借 runuser/su 切换到目标用户。
enum Launcher {
    Direct,
    Runuser(String),
    Su(String),
}

fn resolve_launcher(login_user: &AuthUser) -> Launcher {
    let current_uid = unsafe { libc::getuid() } as i64;
    if login_user.uid == current_uid {
        return Launcher::Direct;
    }
    for bin in ["/usr/sbin/runuser", "/usr/bin/runuser"] {
        if Path::new(bin).exists() {
            return Launcher::Runuser(bin.to_string());
        }
    }
    for bin in ["/bin/su", "/usr/bin/su"] {
        if Path::new(bin).exists() {
            return Launcher::Su(bin.to_string());
        }
    }
    Launcher::Su("su".into())
}

fn detect_shell() -> &'static str {
    if Path::new("/bin/bash").exists() {
        "/bin/bash"
    } else if Path::new("/usr/bin/bash").exists() {
        "/usr/bin/bash"
    } else {
        "/bin/sh"
    }
}

fn env_prefix(config: &AppConfig) -> String {
    let mut prefix = String::new();
    if let Some(lib) = config.tmux_lib_dir() {
        prefix.push_str(&format!(
            "export LD_LIBRARY_PATH={}:${{LD_LIBRARY_PATH:-}}; ",
            shell_quote(&lib.display().to_string())
        ));
    }
    prefix.push_str("export LANG=C.UTF-8; export LC_ALL=C.UTF-8; ");
    prefix
}

/// 把 tmux 调用拼成可交给 `runuser -c` / `su -c` 的单行 shell 命令。
fn shell_command_line(
    config: &AppConfig,
    tmux_bin: &Path,
    args: &[String],
    home: Option<&String>,
) -> String {
    let mut parts = vec![shell_quote(&tmux_bin.display().to_string())];
    parts.extend(args.iter().map(|arg| shell_quote(arg)));
    let body = format!("{}{}", env_prefix(config), parts.join(" "));
    match home {
        Some(dir) => format!("cd {} && {}", shell_quote(dir), body),
        None => body,
    }
}

// ─── 会话查询 ───────────────────────────────────────────────────────────

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

// ─── 会话创建 ───────────────────────────────────────────────────────────

pub async fn create_local_session(
    config: &AppConfig,
    session_id: &str,
    login_user: &AuthUser,
    cols: u32,
    rows: u32,
) -> Result<TerminalSession, String> {
    let tmux_name = format!("rmux_{}", session_id);
    let tmux_bin = config.tmux_path();
    let conf = ensure_tmux_conf(config)?;

    std::fs::create_dir_all(&config.socket_path)
        .map_err(|e| format!("创建 tmux socket 目录失败: {}", e))?;
    let _ = std::fs::set_permissions(&config.socket_path, std::fs::Permissions::from_mode(0o777));

    let shell = detect_shell();
    let target_home = home_for_user(&login_user.user);
    let socket = config.socket_path.join(format!("tmux_{}.sock", session_id));

    info!(
        "准备创建本地 tmux 会话: {}, 窗口: {}x{}, 目标用户: {}",
        tmux_name, cols, rows, login_user.user
    );

    let args: Vec<String> = vec![
        "-f".into(),
        conf.display().to_string(),
        "-u".into(),
        "-S".into(),
        socket.display().to_string(),
        "new-session".into(),
        "-d".into(),
        "-s".into(),
        tmux_name.clone(),
        "-x".into(),
        cols.to_string(),
        "-y".into(),
        rows.to_string(),
        shell.into(),
    ];

    let mut cmd = match resolve_launcher(login_user) {
        Launcher::Direct => {
            let mut cmd = tokio::process::Command::new(&tmux_bin);
            if let Some(home) = &target_home {
                cmd.current_dir(home);
            }
            cmd.args(&args);
            cmd
        }
        Launcher::Runuser(bin) => {
            let line = shell_command_line(config, &tmux_bin, &args, target_home.as_ref());
            let mut cmd = tokio::process::Command::new(bin);
            cmd.args(["-l", &login_user.user, "-c", &line]);
            cmd
        }
        Launcher::Su(bin) => {
            let line = shell_command_line(config, &tmux_bin, &args, target_home.as_ref());
            let mut cmd = tokio::process::Command::new(bin);
            cmd.args(["-", &login_user.user, "-c", &line]);
            cmd
        }
    };

    if let Some(lib) = config.tmux_lib_dir() {
        cmd.env("LD_LIBRARY_PATH", lib);
    }
    cmd.env("SHELL", shell);

    match cmd.output().await {
        Ok(output) if output.status.success() => {
            info!("✅ 成功发起 tmux 创建请求 (用户: {})", login_user.user);
            tokio::time::sleep(tokio::time::Duration::from_millis(600)).await;

            if !check_tmux_session(config, session_id, &tmux_name).await {
                let err_msg = String::from_utf8_lossy(&output.stderr);
                error!("❌ 终端会话创建后未发现或已退出. 详情: {}", err_msg);
                return Err("终端创建失败，请检查系统账户权限".into());
            }

            let now = chrono::Utc::now().to_rfc3339();
            Ok(TerminalSession {
                session_id: session_id.to_string(),
                name: "本地终端".into(),
                owner_uid: login_user.uid,
                owner_user: login_user.user.clone(),
                session_type: "local".into(),
                created_at: now.clone(),
                last_activity: now,
                size: TermSize { cols, rows },
                tmux_session_name: tmux_name,
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

// ─── PTY 客户端 ─────────────────────────────────────────────────────────

/// 一次 WebSocket 连接对应一个 tmux 客户端。tmux session 才是持久层，
/// 这里的 PTY 随连接建立、随连接销毁；重连即新 attach，tmux 会自动全屏重绘。
pub struct PtyIo {
    pub output: tokio::sync::mpsc::Receiver<Vec<u8>>,
    pub control: PtyControl,
}

pub struct PtyControl {
    master: Box<dyn MasterPty + Send>,
    input: std::sync::mpsc::Sender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

impl PtyControl {
    pub fn write_input(&self, data: Vec<u8>) -> bool {
        self.input.send(data).is_ok()
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}

impl Drop for PtyControl {
    fn drop(&mut self) {
        // 关掉 attach 客户端即可，tmux session 本身继续存活。
        let _ = self.killer.kill();
    }
}

const PTY_READ_BUF: usize = 8192;
const PTY_OUTPUT_QUEUE: usize = 256;

/// 在真 PTY 里跑 `tmux attach-session`，让 tmux 自己当渲染器。
/// master 端读到的就是 tmux 为这一个客户端生成的、尺寸正确、状态自洽的终端流。
pub fn attach_pty_session(
    config: &AppConfig,
    session_id: &str,
    session_name: &str,
    login_user: &AuthUser,
    cols: u16,
    rows: u16,
) -> Result<PtyIo, String> {
    let tmux_bin = config.tmux_path();
    let conf = ensure_tmux_conf(config)?;
    let socket = config.socket_path.join(format!("tmux_{}.sock", session_id));
    let target_home = home_for_user(&login_user.user);

    // `-d` 顶掉同会话的其它客户端：任一时刻只有一个客户端，
    // 尺寸永远等于当前浏览器窗口，不会出现多客户端互相压缩导致的重排抖动。
    // `-f` 只在 tmux server 首次启动时生效；恢复出来的旧 server 不会重读配置。
    // attach 前显式关闭 tmux 鼠标捕获，现有会话也会立即恢复浏览器的原生
    // 右键菜单和 xterm 本地选择；否则右键会打开 tmux 菜单，拖选也会在松手时取消。
    let args: Vec<String> = vec![
        "-f".into(),
        conf.display().to_string(),
        "-u".into(),
        "-S".into(),
        socket.display().to_string(),
        "set-option".into(),
        "-g".into(),
        "mouse".into(),
        "off".into(),
        ";".into(),
        "attach-session".into(),
        "-d".into(),
        "-t".into(),
        session_name.into(),
    ];

    let pty = NativePtySystem::default();
    let pair = pty
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("创建 PTY 失败: {}", e))?;

    let mut cmd = match resolve_launcher(login_user) {
        Launcher::Direct => {
            let mut cmd = CommandBuilder::new(&tmux_bin);
            cmd.args(&args);
            cmd
        }
        Launcher::Runuser(bin) => {
            let line = shell_command_line(config, &tmux_bin, &args, target_home.as_ref());
            let mut cmd = CommandBuilder::new(bin);
            cmd.args(["-l", &login_user.user, "-c", &line]);
            cmd
        }
        Launcher::Su(bin) => {
            let line = shell_command_line(config, &tmux_bin, &args, target_home.as_ref());
            let mut cmd = CommandBuilder::new(bin);
            cmd.args(["-", &login_user.user, "-c", &line]);
            cmd
        }
    };

    // 外层 TERM 决定 tmux 如何和浏览器端 xterm.js 对话；RGB 能力已在 tmux 配置里
    // 按这个 TERM 名声明，缺了它 claude code 之类的真彩色会降级成 256 色。
    cmd.env("TERM", "xterm-256color");
    cmd.env("LANG", "C.UTF-8");
    cmd.env("LC_ALL", "C.UTF-8");
    if let Some(lib) = config.tmux_lib_dir() {
        cmd.env("LD_LIBRARY_PATH", lib);
    }
    if let Some(home) = &target_home {
        cmd.cwd(home);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("启动 tmux 客户端失败: {}", e))?;
    let killer = child.clone_killer();
    // slave 必须尽早释放，否则子进程退出后 master 端读不到 EOF。
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("获取 PTY 读端失败: {}", e))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("获取 PTY 写端失败: {}", e))?;

    // 这些线程都长时间阻塞在 read/recv 上，用独立 OS 线程而不是 tokio 的
    // blocking 池——后者是给短任务用的，长驻会把池占死。
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(PTY_OUTPUT_QUEUE);
    std::thread::spawn(move || {
        let mut buf = [0u8; PTY_READ_BUF];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if out_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                // slave 关闭后 Linux 上 master 读会返回 EIO，等价于 EOF。
                Err(_) => break,
            }
        }
    });

    let (in_tx, in_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        while let Ok(data) = in_rx.recv() {
            if writer.write_all(&data).is_err() || writer.flush().is_err() {
                break;
            }
        }
    });

    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(PtyIo {
        output: out_rx,
        control: PtyControl {
            master: pair.master,
            input: in_tx,
            killer,
        },
    })
}

// ─── 粘贴 ───────────────────────────────────────────────────────────────

/// 大段文本走 tmux 缓冲区而不是逐字节写 PTY：`paste-buffer -p` 会带上
/// bracketed paste 标记，程序能把整段识别为一次粘贴而非连续按键。
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

// ─── 会话销毁 ───────────────────────────────────────────────────────────

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
