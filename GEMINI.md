# Rmux 项目指南

## 界面语言

- **强制要求：** 用户界面（UI）必须使用 **中文**。所有按钮、标签、提示信息和错误消息应以中文显示。

## 运行环境

- **目标环境：** 应用面向本地 Linux/Ubuntu 环境运行。
- **路径处理：** 必须优先使用 `RMUX_APPDIR`、`RMUX_DATA_DIR`、`RMUX_UI_DIR`、`RMUX_TMUX` 和 `RMUX_TMUX_LIB_DIR` 环境变量定位应用目录、数据目录、前端资源目录和 tmux 相关路径。
