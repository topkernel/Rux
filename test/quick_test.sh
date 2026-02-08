#!/bin/bash
# 快速测试脚本 - 使用正确的 QEMU 参数

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

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

echo "启动 Rux OS (使用 OpenSBI)..."
echo "提示：使用 Ctrl+A, X 退出 QEMU"

# 使用 -bios default（OpenSBI）或不指定 -bios（默认也是 OpenSBI）
qemu-system-riscv64 \
    -M virt \
    -cpu rv64 \
    -m 2G \
    -nographic \
    -serial mon:stdio \
    -kernel "$KERNEL_BINARY" \
    -smp 1
