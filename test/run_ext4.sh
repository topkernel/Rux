#!/bin/bash
# 测试 ext4 文件系统
#
# 使用 QEMU virtio-blk 提供块设备

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# 检查镜像文件是否存在
IMAGE_FILE="test/disk.img"
if [ ! -f "$IMAGE_FILE" ]; then
    echo "Error: Disk image not found at $IMAGE_FILE"
    echo "Run ./test/create_ext4_image.sh to create the image first."
    exit 1
fi

# 默认使用 debug 模式（编译更快）
BUILD_MODE="${BUILD_MODE:-debug}"
KERNEL_BINARY="target/riscv64gc-unknown-none-elf/${BUILD_MODE}/rux"

# 检查内核是否已构建
if [ ! -f "$KERNEL_BINARY" ]; then
    echo "内核二进制文件不存在，正在构建 (${BUILD_MODE})..."
    if [ "$BUILD_MODE" = "release" ]; then
        cargo build --package rux --features riscv64 --release
    else
        cargo build --package rux --features riscv64
    fi
fi

# 构建QEMU命令
QEMU_CMD="qemu-system-riscv64 \
    -M virt \
    -cpu rv64 \
    -m 2G \
    -nographic \
    -serial mon:stdio \
    -drive file=${IMAGE_FILE},if=none,format=raw,id=hd0 \
    -device virtio-blk-device,drive=hd0,bus=virtio-mmio-bus.0"

echo "启动内核: 带 ext4 镜像"
echo "镜像: $IMAGE_FILE"

# 运行QEMU
eval $QEMU_CMD
