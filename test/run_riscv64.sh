#!/bin/bash
# QEMU运行脚本 for Rux Kernel (riscv64)
# 支持单核和双核模式

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# 默认使用 debug 模式（编译更快）
BUILD_MODE="${BUILD_MODE:-debug}"
KERNEL_BINARY="target/riscv64gc-unknown-none-elf/${BUILD_MODE}/rux"

# SMP 支持：默认启用单核
SMP="${SMP:-1}"

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
    -serial mon:stdio"

# 添加SMP支持
if [ "$SMP" -gt 1 ]; then
    QEMU_CMD="$QEMU_CMD -smp $SMP"
    echo "启动内核: ${SMP}核模式"
else
    echo "启动内核: 单核模式"
fi

# 添加内核路径
QEMU_CMD="$QEMU_CMD -kernel \"$KERNEL_BINARY\""

# 运行QEMU
eval $QEMU_CMD
