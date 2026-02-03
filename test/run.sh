#!/bin/bash
# QEMU运行脚本 for Rux Kernel (aarch64)

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

QEMU_SYSTEM_AARCH64="qemu-system-aarch64"
KERNEL_BINARY="target/aarch64-unknown-none/release/rux"

# 检查内核是否已构建
if [ ! -f "$KERNEL_BINARY" ]; then
    echo "内核二进制文件不存在，正在构建..."
    cargo build --package rux --features aarch64 --release
fi

# 运行QEMU
# 使用 virt 机器类型，这是QEMU为虚拟化优化的ARM机器
# -m 2G: 分配2GB内存
# -nographic: 不使用图形界面，使用串口
# -kernel: 加载内核二进制文件
$QEMU_SYSTEM_AARCH64 \
    -M virt \
    -cpu cortex-a57 \
    -m 2G \
    -nographic \
    -kernel "$KERNEL_BINARY"
