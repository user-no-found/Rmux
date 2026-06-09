#!/bin/bash
# Rmux 一键构建脚本
# 构建流程：编译 Rust → 编译前端 → fnpack 打包
# 用法：./build_fpk.sh
# 默认使用 build_fpk/manifest 中的现有版本号，不自动递增

set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BUILD_DIR="${SCRIPT_DIR}/build_fpk"

if ! command -v npm >/dev/null 2>&1; then
    for candidate in /vol1/1000/sun/.nvm/versions/node/*/bin/npm; do
        if [ -x "$candidate" ]; then
            export PATH="$(dirname "$candidate"):$PATH"
            break
        fi
    done
fi

echo "========================================="
echo "  Rmux 构建脚本"
echo "========================================="

# 检查 manifest 版本格式
VERSION=$(grep "^version" "${BUILD_DIR}/manifest" | awk '{print $3}')
if ! echo "$VERSION" | grep -q '^[0-9]\+\.[0-9]\+\.[0-9]\+$'; then
    echo "❌ 错误: manifest 版本格式不正确（当前: $VERSION），必须为 x.y.z 格式"
    echo "   请先手动修改 build_fpk/manifest 中的 version 字段"
    exit 1
fi

ICON_TAG="v${VERSION//./_}"
echo "📦 版本: $VERSION"

# Step 1: 编译 Rust 后端
echo ""
echo "[1/5] 生成应用图标..."
ICON_DIR="${BUILD_DIR}/app/ui/images"
mkdir -p "${ICON_DIR}"
find "${ICON_DIR}" -maxdepth 1 -type f -name '*.png' -delete
rm -f "${BUILD_DIR}/ICON.PNG" "${BUILD_DIR}/ICON_256.PNG" "${BUILD_DIR}/app/www/icon.png"
python3 "${SCRIPT_DIR}/scripts/generate_icons.py"
for size in 16 24 32 48 64 72 96 128 256; do
    cp "${ICON_DIR}/rmux_${size}.png" "${ICON_DIR}/rmux_${ICON_TAG}_${size}.png"
    cp "${ICON_DIR}/icon_${size}.png" "${ICON_DIR}/icon_${ICON_TAG}_${size}.png"
done
cp "${ICON_DIR}/rmux_{0}.png" "${ICON_DIR}/rmux_${ICON_TAG}_{0}.png"
cp "${ICON_DIR}/icon_{0}.png" "${ICON_DIR}/icon_${ICON_TAG}_{0}.png"
sed -i -E "s|\"icon\": \"images/rmux([^\\\"]*)\\{0\\}\\.png\"|\"icon\": \"images/rmux_${ICON_TAG}_{0}.png\"|g" "${BUILD_DIR}/app/ui/config"
sed -i -E "s|\"url\": \"/app/rmux/[^\\\"]*\"|\"url\": \"/app/rmux/?v=${VERSION}\"|g" "${BUILD_DIR}/app/ui/config"
echo "  ✅ 图标已重新生成"

# Step 2: 编译 Rust 后端
echo ""
echo "[2/5] 编译 Rust 后端..."
cd "${SCRIPT_DIR}/backend"
cargo build --release
echo "  ✅ 后端编译完成"

# Step 3: 编译 Vue 前端
echo ""
echo "[3/5] 编译 Vue 前端..."
cd "${SCRIPT_DIR}/frontend"
VITE_APP_BUILD_VERSION="$VERSION" npm run build
echo "  ✅ 前端编译完成"

# Step 4: 同步文件到打包目录
echo ""
echo "[4/5] 同步文件到 build_fpk..."
# 后端二进制
cp "${SCRIPT_DIR}/backend/target/release/rmux" "${BUILD_DIR}/app/server/rmux"
chmod 755 "${BUILD_DIR}/app/server/rmux"
# 确保内置二进制也有执行权限
chmod +x "${BUILD_DIR}/app/server/bin/"* 2>/dev/null || true
echo "  ✅ 二进制及内置工具已同步并设置权限"

# Web 前端（到 app/www/）
rm -rf "${BUILD_DIR}/app/www"
mkdir -p "${BUILD_DIR}/app/www"
cp -r "${SCRIPT_DIR}/ui/index.html" "${BUILD_DIR}/app/www/"
cp -r "${SCRIPT_DIR}/ui/assets" "${BUILD_DIR}/app/www/"
if [ -f "${SCRIPT_DIR}/ui/icon.png" ]; then
    cp -r "${SCRIPT_DIR}/ui/icon.png" "${BUILD_DIR}/app/www/"
fi
# 同步桌面入口图标到 www/images/，让 fnOS 应用商店请求的 /app/rmux/images/*.png 能命中
mkdir -p "${BUILD_DIR}/app/www/images"
cp "${BUILD_DIR}/app/ui/images/"*.png "${BUILD_DIR}/app/www/images/"
echo "  ✅ Web 前端已同步到 app/www/"

# 给脚本设置可执行权限
chmod 755 "${BUILD_DIR}/cmd/main" "${BUILD_DIR}"/cmd/*_init "${BUILD_DIR}"/cmd/*_callback 2>/dev/null || true

echo "  ✅ 文件同步完成"

# Step 5: fnpack 打包
echo ""
echo "[5/5] 构建 fpk..."
cd "${SCRIPT_DIR}"

# 清理旧包
rm -f "${BUILD_DIR}/rmux.fpk"
rm -f "${BUILD_DIR}/app.tgz"

# 用 fnpack 打包（注意：fnpack 输出到当前工作目录）
fnpack build --directory "${BUILD_DIR}"

# 移动 fpk 到 build_fpk/
if [ -f rmux.fpk ]; then
    mv rmux.fpk "${BUILD_DIR}/"
fi

echo ""
echo "========================================="
echo "  ✅ 构建完成!"
echo "  fpk: ${BUILD_DIR}/rmux.fpk"
echo "  大小: $(ls -lh "${BUILD_DIR}/rmux.fpk" | awk '{print $5}')"
echo "  版本: $VERSION"
echo "  提示: 安装前确保 fnOS App Center 中无旧版本冲突"
echo "========================================="
