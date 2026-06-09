# Rmux FPK 打包指南

> 飞牛 fnOS 原生应用打包方法，基于 aicore-web 的 FPK 打包教程实践总结。

## 目录约定

```
build_fpk/                          # 打包源目录
├── app/
│   ├── server/                     # 后端二进制
│   │   ├── rmux                  # Rust 主程序
│   │   ├── bin/
│   │   │   ├── tmux                # 内置 tmux
│   │   │   └── sshpass             # 内置 sshpass
│   │   └── lib/
│   │       └── libutempter.so.0
│   ├── www/                        # Web 前端（与 ui/ 分开）
│   │   ├── index.html
│   │   └── assets/
│   │       ├── app.css
│   │       └── app.js
│   └── ui/                         # fnOS 桌面入口
│       ├── config                  # 桌面入口配置
│       └── images/
│           ├── icon_64.png
│           └── icon_256.png
├── cmd/                            # 生命周期脚本
│   ├── main                        # start / stop / status
│   ├── common                      # 公共函数库
│   ├── install_init
│   ├── install_callback
│   ├── uninstall_init
│   ├── uninstall_callback
│   ├── upgrade_init
│   ├── upgrade_callback
│   ├── config_init
│   └── config_callback
├── config/                         # fnOS 应用权限
│   ├── privilege
│   └── resource
├── wizard/                         # 安装向导（可为空）
│   └── .keep
├── manifest                        # 应用元信息
├── ICON.PNG                        # 64x64
├── ICON_256.PNG                    # 256x256
└── LICENSE
```

## Manifest 格式

**关键：** 不要有多余字段。参考 aicore-web 的精简格式：

```text
appname               = rmux
version               = 0.1.0
display_name          = Rmux
desc                  = Web 终端管理，支持本地终端，主题自定义
platform              = x86
source                = thirdparty
maintainer            = user-no-found
distributor           = user-no-found
desktop_uidir         = ui               # 指向 app/ui/
desktop_applaunchname = rmux.Application
service_port          = 18732            # 避免使用常见端口
checkport             = true
```

**注意：**
- ❌ 不要加 `arch = x86_64` 等架构字段（`platform = x86` 已足够）
- ❌ 不要加 `install_type = root`（会导致系统拒绝安装）
- ❌ 不要加 `os_min_version`
- ❌ 不要加 `maintainer_url`、`distributor_url`、`helpurl`、`changelog`
- ⚠️ **版本必须用 `0.0.x` 序列**，不能用 `1.x.x`
- ⚠️ **只有在发布新版本时才手动修改版本号**；需要覆盖同一发布时保持版本号不变

## 配置文件

### config/privilege

```json
{
    "defaults": {
        "run-as": "package"
    }
}
```

**关键：** 使用 `run-as: "package"`，不要写 username/groupname，不要用 `"root"`。

### config/resource

```json
{
    "data-share": {
        "shares": [
            {
                "name": "rmux",
                "permission": { "rw": ["rmux"] }
            },
            {
                "name": "rmux/data",
                "permission": { "rw": ["rmux"] }
            }
        ]
    }
}
```

**关键：** 不要用空的 `{}`，必须用 `data-share` 格式。

## 桌面入口配置

### app/ui/config

```json
{
    ".url": {
        "rmux.Application": {
            "title": "终端",
            "icon": "images/icon_{0}.png",
            "type": "url",
            "protocol": "http",
            "port": "18732",
            "url": "/",
            "allUsers": false
        }
    }
}
```

**关键：**
- `"type": "url"` 而非 `"iframe"`
- 必须包含 `"url": "/"` 字段
- 端口号必须与 manifest 中的 `service_port` 一致

## cmd/main 生命周期脚本

采用 aicore-web 风格的稳健路径解析：

```bash
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PKG_ROOT=$(CDPATH= cd -- "${SCRIPT_DIR}/.." && pwd)
APP_DEST="${TRIM_APPDEST:-${PKG_ROOT}/app}"
# TRIM_APPDEST 是 fnOS 注入的变量，指向 app.tgz 的解压目录
```

二进制查找路径优先顺序：
1. `${TRIM_APPDEST}/server/rmux`（fnOS 实机路径）
2. `${PKG_ROOT}/server/rmux`（本地模拟 fallback）

## 构建流程

```bash
# 1. 编译 Rust 后端
cd rmux/backend
cargo build --release

# 2. 编译 Vue 前端
cd ../frontend
npm run build          # 输出到 rmux/ui/

# 3. 用 fnpack 打包
cd ../build_fpk
# 确保以下文件到位：
#   app/server/rmux           ← 来自 target/release/rmux
#   app/www/index.html + assets/  ← 来自 ../ui/
#   app/ui/config + images/     ← 桌面入口文件
#   cmd/*                       ← 生命周期脚本
#   config/*                    ← 权限配置
#   wizard/.keep                ← 安装向导
#   manifest                    ← 应用元信息
#   ICON.PNG + ICON_256.PNG     ← 应用图标
#   LICENSE                     ← 许可证

fnpack build --directory .

# 4. 验证
# 检查外层 manifest
mkdir -p /tmp/verify && tar -xzf rmux.fpk -C /tmp/verify
cat /tmp/verify/manifest
# 检查 app.tgz 内容
tar -tzf /tmp/verify/app.tgz
```

## 常见失败原因

| 现象 | 原因 | 修复 |
|------|------|------|
| 应用包不符合系统要求 | manifest 包含多余字段（`arch`、`install_type`、`os_min_version`） | 精简 manifest |
| 应用包不符合系统要求 | 版本号不是 `0.0.x` 格式 | 改为 `0.0.x` |
| 应用包不符合系统要求 | `config/privilege` 包含 username/groupname 或 `run-as: root` | 用简洁格式 `run-as: package` |
| 应用包不符合系统要求 | `config/resource` 为空 `{}` | 用 `data-share` 格式 |
| 安装成功但启动失败 | cmd/main 路径解析错误 | 优先使用 `TRIM_APPDEST` |
| 安装成功但无法打开 | app/ui/config 类型或字段错误 | 使用 `type: url` + `url: "/"` |
| 升级后仍用旧脚本 | 发布了新版本但未修改 version | 发布新版本前手动更新 version |

## 参考

- 飞牛官方 fnpack 文档：https://developer.fnnas.com/docs/cli/fnpack
- aicore-web 打包模板：参考本地 AICore-OS 仓库中的 `apps/aicore-web/packaging/fnos/`
- 打包教程：参考本地 fnOS FPK 打包文档
