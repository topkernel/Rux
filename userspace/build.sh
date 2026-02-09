#!/bin/bash
#
# Rux 用户程序构建脚本
#
# 这个脚本会临时禁用根目录的 .cargo/config.toml 来避免配置冲突

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

# 备份根目录的 cargo config（如果存在）
CARGO_CONFIG="$ROOT_DIR/.cargo/config.toml"
BACKUP_CONFIG="$ROOT_DIR/.cargo/config.toml.backup"

if [ -f "$CARGO_CONFIG" ]; then
    echo "备份根目录的 cargo 配置..."
    cp "$CARGO_CONFIG" "$BACKUP_CONFIG"
    # 临时移除配置
    mv "$CARGO_CONFIG" "$CARGO_CONFIG.disabled"
fi

# 清理构建函数
cleanup() {
    if [ -f "$BACKUP_CONFIG" ]; then
        echo "恢复根目录的 cargo 配置..."
        mv "$CARGO_CONFIG.disabled" "$CARGO_CONFIG"
        rm -f "$BACKUP_CONFIG"
    fi
}

# 设置 trap 确保清理
trap cleanup EXIT

# 进入用户程序目录
cd "$SCRIPT_DIR"

# 构建
echo "构建用户程序..."
# 添加链接器脚本参数，将用户程序链接到用户空间地址
RUSTFLAGS="-C link-arg=-Tuser.ld" cargo build --release "$@"

# 显示输出文件
echo ""
echo "构建完成！输出文件："
find target/riscv64gc-unknown-none-elf/release -type f -executable | while read file; do
    echo "  - $file ($(stat -f%z "$file" 2>/dev/null || stat -c%s "$file") bytes)"
done
