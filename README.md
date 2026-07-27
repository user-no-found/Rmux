# Rmux · 飞牛 fnOS Web 终端

为飞牛 fnOS 打造的浏览器终端应用，解决「在 NAS 上用 Web 终端跑 CLI 工具」时的两个体验问题。

## 为什么做这个？

平时在 NAS 上用 Web 终端，自己遇到过两个一直没能很顺手解决的小问题：

- **`Ctrl + Enter` 无法换行**：在 Claude Code、Codex 等 CLI 工具里需要多行输入时，希望 `Ctrl + Enter` 能插入换行而不是直接提交。
- **图片无法传入终端**：在用 AI / CLI 工具时，经常想把一张截图作为输入交给终端里的程序，但 Web 终端本身没有「把图片送进终端」的通道。

这两个点恰好都是我日常用得最多的场景，于是顺手写了这个项目，把这两件事处理掉。

## 它能做什么

- ✅ **`Ctrl + Enter` 正确发送 LF**：在 CLI 工具里正常换行，不会误触提交
- ✅ **图片可以直接传入终端**：复制截图后在终端里 `Ctrl + V`，自动上传到后端并把文件路径填入命令行，方便交给 AI / CLI 处理
- ✅ **会话持久化**：基于内置 tmux。关闭浏览器、断网重连、后端进程重启或崩溃，终端状态都原样保留
  （例外：FPK 升级和主机重启会结束所有会话 —— 升级要替换内置 tmux 二进制，新旧 server 协议不兼容，只能重来）
- ✅ **多标签会话**：一个浏览器窗口同时跑多个终端，标签可重命名
- ✅ **右键粘贴**：右键即粘贴；选中文本后右键则是复制，符合桌面终端习惯
- ✅ **fnOS 原生集成**：作为 FPK 应用安装，桌面图标直接打开，自动识别系统用户身份

### 尚未接线的部分

设置页可以保存配色、字号、光标样式，后端也有完整的背景图上传 / URL 接口，但**终端本身目前还没有读取这些设置**——`TerminalView.vue` 用的仍是硬编码的字号和调色板。换句话说这套设置目前只存不用。如果你是冲着主题自定义来的，请先知悉。

## 技术栈

| 层 | 选型 |
|---|---|
| 后端 | Rust + Axum + Tokio + WebSocket + SQLite |
| 前端 | Vue 3 + Vite + xterm.js |
| 终端核心 | 内置 tmux（会话持久化） |
| 打包 | fnpack（飞牛官方 FPK 格式） |

后端编译后是一个单文件二进制，连同 tmux 一起打包进 FPK，安装到 fnOS 即可使用，无需任何依赖。

## 开发说明

开发过程中，编码工作主要由 **GPT-5.5** 和 **DeepSeek-V4-Pro** 两个模型完成。

## 安装使用

1. 本地手动安装 `rmux.fpk`
2. 安装后从 fnOS 桌面点击「**终端**」图标启动
3. 首次启动可选择设置访问密码，或跳过进入公开模式
4. 默认服务端口：`18732`

## 从源码构建

```bash
# 一键构建（需要 cargo / node / fnpack）
./build_fpk.sh
```

构建脚本会依次完成：生成图标 → 编译 Rust 后端 → 编译 Vue 前端 → 同步文件 → `fnpack` 打包。
产物输出在 `build_fpk/rmux.fpk`。

> ⚠️ `build_fpk.sh` 会直接使用 `build_fpk/manifest` 里的现有版本号。只有在你明确要发新版本时，才手动修改 `version`。详细打包注意事项见 [FPK_PACKAGING.md](./FPK_PACKAGING.md)。

## 目录结构

```
rmux/
├── backend/       # Rust 后端（Axum + tmux 会话管理）
├── frontend/      # Vue 3 前端（xterm.js 终端 UI）
├── ui/            # 前端构建产物（vite build 输出）
├── build_fpk/     # FPK 打包源（manifest / cmd / config / 图标）
├── scripts/       # 辅助脚本（图标生成等）
└── build_fpk.sh   # 一键构建脚本
```

## License

MIT License. 见 [LICENSE](./LICENSE)。
