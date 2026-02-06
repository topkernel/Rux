#!/bin/bash
# RISC-V 测试脚本 for Rux Kernel

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# 默认使用 debug 模式
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

echo "=========================================="
echo "  Rux OS - RISC-V 64-bit Test"
echo "=========================================="
echo ""

# 运行 QEMU（10秒超时）
timeout 10 qemu-system-riscv64 \
    -machine virt \
    -cpu rv64 \
    -smp 1 \
    -m 2G \
    -nographic \
    -bios /usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin \
    -kernel "$KERNEL_BINARY" \
    -serial mon:stdio 2>&1

echo ""
echo "测试完成"
