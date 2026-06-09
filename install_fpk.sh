#!/bin/bash
# Rmux FPK 手动安装脚本
# 用于 appcenter-cli 不可用时

set -e
FPK="$1"
APPDIR="/var/apps/rmux"
CENTERDIR="/usr/local/apps/@appcenter/rmux"

if [ -z "$FPK" ]; then
    echo "Usage: $0 <rmux.fpk>"
    exit 1
fi

echo "=== 解压 fpk ==="
TMPDIR=$(mktemp -d)
gzip -dc "$FPK" | dd bs=1 skip=100 2>/dev/null > "$TMPDIR/app_data.bin"

echo "=== 解析条目 ==="
# Simple extraction - first entry is the binary, rest are files
python3 -c "
import struct, os, sys

data = open('$TMPDIR/app_data.bin', 'rb').read()
pos = 0
file_idx = 0
outdir = '$TMPDIR/extracted'

while pos + 512 <= len(data):
    hdr = data[pos:pos+512]
    parts = hdr[:100].split(b'\\x00')
    if len(parts) < 7:
        break
    try:
        mode = int(parts[0].decode('ascii'), 8)
        size = int(parts[3].decode('ascii'), 8) if parts[3] else 0
        flag = parts[6].decode('ascii', 'replace').strip()
    except:
        break
    
    if flag == '5':
        # Directory
        os.makedirs(f'{outdir}/entry_{file_idx:04d}', exist_ok=True)
        os.makedirs(f'{outdir}/entry_{file_idx:04d}', exist_ok=True)
    else:
        # File
        fpath = f'{outdir}/entry_{file_idx:04d}'
        fdata = data[pos+512:pos+512+size]
        open(fpath, 'wb').write(fdata)
        os.chmod(fpath, mode)
    
    file_idx += 1
    data_blocks = (size + 511) // 512
    pos += 512 + data_blocks * 512

print(f'Extracted {file_idx} entries')
"

echo "=== 安装到目标位置 ==="
rm -rf "$APPDIR" "$CENTERDIR"
mkdir -p "$APPDIR" "$CENTERDIR"

# Copy extracted files to the right locations
cp -r "$TMPDIR/extracted/"* "$APPDIR/"

# Copy target/ to centerdir
if [ -d "$APPDIR/target" ]; then
    cp -r "$APPDIR/target/"* "$CENTERDIR/"
fi

# Create socket
touch "$CENTERDIR/rmux.sock"
chmod 666 "$CENTERDIR/rmux.sock"

echo ""
echo "✅ Rmux 安装完成!"
echo "   应用目录: $APPDIR"
echo "   中心目录: $CENTERDIR"
echo ""
echo "启动: $APPDIR/cmd/main start"
