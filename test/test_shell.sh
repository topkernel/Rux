#!/bin/bash
#
# Shell 测试脚本 for Rux Kernel (riscv64)
#
# 用法：./test/test_shell.sh
#
# 说明：
# - 启动内核并从 rootfs 加载 shell
# - 支持交互式手动测试
# - 按 Ctrl+A 然后 X 退出 QEMU

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

BUILD_MODE="${BUILD_MODE:-debug}"
KERNEL_BINARY="target/riscv64gc-unknown-none-elf/${BUILD_MODE}/rux"
ROOTFS_IMAGE="test/rootfs.img"

# 检查内核是否存在
if [ ! -f "$KERNEL_BINARY" ]; then
    echo "错误：内核二进制文件不存在: $KERNEL_BINARY"
    echo "请先运行 'make build' 编译内核"
    exit 1
fi

# 检查 rootfs 是否存在
if [ ! -f "$ROOTFS_IMAGE" ]; then
    echo "错误：rootfs 镜像不存在: $ROOTFS_IMAGE"
    echo "请先运行 'make rootfs' 创建 rootfs"
    exit 1
fi

echo "=========================================="
echo "Rux OS - Shell 测试 (从 rootfs)"
echo "=========================================="
echo "内核: $KERNEL_BINARY"
echo "Rootfs: $ROOTFS_IMAGE"
echo ""
echo "提示："
echo "  - Ctrl+A 然后 X 退出 QEMU"
echo "  - 支持的命令: echo, help, exit"
echo "=========================================="
echo ""

# 使用 VirtIO-Blk 设备提供 rootfs
# 不使用 -bios none，让 QEMU 使用默认 OpenSBI
# 添加 disable-modern=on 强制使用 legacy VirtIO 接口
qemu-system-riscv64 \
    -M virt \
    -cpu rv64 \
    -m 2G \
    -nographic \
    -serial mon:stdio \
    -drive file="$ROOTFS_IMAGE",if=none,format=raw,id=rootfs \
    -device virtio-blk-device,drive=rootfs,disable-modern=on \
    -kernel "$KERNEL_BINARY" \
    -append "root=/dev/vda rw console=ttyS0 init=/bin/sh"
