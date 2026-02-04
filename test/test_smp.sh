#!/bin/bash
# SMP 启动测试脚本
#
# 测试 Rux 内核的双核启动功能

set -e

echo "=========================================="
echo "  Rux 内核 SMP 启动测试"
echo "=========================================="
echo ""

# 检查内核是否已编译
if [ ! -f "target/aarch64-unknown-none/debug/rux" ]; then
    echo "错误: 内核未编译"
    echo "请先运行: make build"
    exit 1
fi

echo "启动 QEMU (2 核模式)..."
echo "预期输出:"
echo "  [CPU0 up]"
echo "  [CPU1 up]"
echo "  SMP: 2 CPUs online"
echo ""

qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a57 \
    -m 2G \
    -smp 2 \
    -nographic \
    -serial mon:stdio \
    -kernel target/aarch64-unknown-none/debug/rux

echo ""
echo "测试完成"
