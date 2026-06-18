# Rmux · Web Terminal

Rmux 是一个面向本地 Linux/Ubuntu 环境的浏览器终端应用，用来在 Web 页面里运行常用 CLI 工具，并保留接近桌面终端的输入体验。

## 为什么做这个？

平时在 Web 终端里使用 Claude Code、Codex 等 CLI 工具时，常见的两个操作不够顺手：

- **`Ctrl + Enter` 无法换行**：多行输入时，希望 `Ctrl + Enter` 插入换行，而不是直接提交。
- **图片无法传入终端**：需要把截图交给终端里的程序时，希望复制图片后能直接粘贴，并自动得到后端保存后的文件路径。

Rmux 把这些终端交互补上，同时用 tmux 保持会话状态，浏览器刷新或重新连接后仍能回到原来的终端。

## 它能做什么

- ✅ **`Ctrl + Enter` 正确发送 LF**：在 CLI 工具里正常换行，不会误触提交
- ✅ **图片可以直接传入终端**：复制截图后在终端里 `Ctrl + V`，自动上传到后端并把文件路径填入命令行
- ✅ **会话持久化**：基于 tmux，关闭浏览器、断网重连，终端状态原样保留
- ✅ **多标签会话**：一个浏览器窗口同时跑多个终端，标签可重命名
- ✅ **主题自定义**：内置多套配色、字体大小、光标样式、自定义背景图（上传 / URL）
- ✅ **右键复制粘贴**：选中即复制，右键即粘贴，符合桌面终端习惯
- ✅ **本地用户识别**：服务端自动使用当前 Linux 登录用户作为终端运行身份

## 技术栈

| 层 | 选型 |
|---|---|
| 后端 | Rust + Axum + Tokio + WebSocket + SQLite |
| 前端 | Vue 3 + Vite + xterm.js |
| 终端核心 | tmux（会话持久化） |

后端是一个 Rust 服务，前端由 Vite 构建到 `ui/` 目录。默认情况下，运行后会在本机 `18732` 端口提供 Web 终端。

## 开发说明

开发过程中，编码工作主要由 **GPT-5.5** 和 **DeepSeek-V4-Pro** 两个模型完成。

## 从源码构建

```bash
# 构建前端
cd frontend
npm install
npm run build

# 构建后端
cd ../backend
cargo build --release
```

前端产物输出到仓库根目录的 `ui/`。后端默认读取 `../ui` 或通过环境变量指定的静态资源目录。

## 运行

```bash
cd backend
RMUX_APPDIR=.. \
RMUX_DATA_DIR=../var \
RMUX_UI_DIR=../ui \
cargo run --release
```

启动后访问：

```text
http://127.0.0.1:18732
```

可用环境变量：

| 变量 | 说明 | 默认值 |
|---|---|---|
| `RMUX_APPDIR` | 应用根目录 | `.` |
| `RMUX_DATA_DIR` | 数据、日志、上传文件目录 | `var` |
| `RMUX_UI_DIR` | 前端静态资源目录 | `$RMUX_APPDIR/ui` |
| `RMUX_TMUX` | 自定义 tmux 可执行文件路径 | `tmux` |
| `RMUX_TMUX_LIB_DIR` | 自定义 tmux 依赖库目录 | 未设置 |
| `RMUX_PORT` | HTTP 服务端口 | `18732` |
| `RMUX_HOST` | HTTP 监听地址 | `0.0.0.0` |
| `JWT_SECRET` | JWT 密钥 | 自动生成/默认空值 |

## 目录结构

```text
rmux/
├── backend/       # Rust 后端（Axum + tmux 会话管理）
├── frontend/      # Vue 3 前端（xterm.js 终端 UI）
├── ui/            # 前端构建产物（vite build 输出）
├── assets/        # 源图标等静态素材
└── scripts/       # 辅助脚本（图标生成等）
```

## License

MIT License. 见 [LICENSE](./LICENSE)。
