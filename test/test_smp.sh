#!/bin/bash
# SMP 多核测试脚本

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

BUILD_MODE="${BUILD_MODE:-debug}"
KERNEL_BINARY="target/riscv64gc-unknown-none-elf/${BUILD_MODE}/rux"

# 检查内核是否已构建
if [ ! -f "$KERNEL_BINARY" ]; then
    echo "内核二进制文件不存在，正在构建 (${BUILD_MODE})..."
    cargo build --package rux --features riscv64,unit-test
fi

echo "=========================================="
echo "SMP 多核测试"
echo "=========================================="
echo "配置: 4 CPU cores"
echo "提示：使用 Ctrl+A, X 退出 QEMU"
echo ""

# 使用 -smp 4 启用 4 个 CPU 核心
qemu-system-riscv64 \
    -M virt \
    -cpu rv64 \
    -m 2G \
    -nographic \
    -serial mon:stdio \
    -kernel "$KERNEL_BINARY" \
    -smp 4
