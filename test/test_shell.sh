#!/bin/bash
#
# Shell 测试脚本 for Rux Kernel (riscv64)
#
# 用法：./test/test_shell.sh
#
# 说明：
# - 启动内核并以 shell 作为 init 进程
# - 支持交互式手动测试
# - 按 Ctrl+A 然后 X 退出 QEMU

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

BUILD_MODE="${BUILD_MODE:-debug}"
KERNEL_BINARY="target/riscv64gc-unknown-none-elf/${BUILD_MODE}/rux"

# 检查内核是否存在
if [ ! -f "$KERNEL_BINARY" ]; then
    echo "错误：内核二进制文件不存在: $KERNEL_BINARY"
    echo "请先运行 'make build' 编译内核"
    exit 1
fi

echo "=========================================="
echo "Rux OS - Shell 测试"
echo "=========================================="
echo "内核: $KERNEL_BINARY"
echo ""
echo "提示："
echo "  - Ctrl+A 然后 X 退出 QEMU"
echo "  - 支持的命令: echo, help, exit"
echo "=========================================="
echo ""

QEMU_CMD="qemu-system-riscv64 \
    -M virt \
    -cpu rv64 \
    -m 2G \
    -nographic \
    -serial mon:stdio \
    -bios none \
    -kernel $KERNEL_BINARY \
    -append \"root=/dev/ram0 rw console=ttyS0 init=/shell\""

eval $QEMU_CMD
