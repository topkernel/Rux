#!/bin/bash
# SMP 启动测试脚本
#
# 测试 Rux 内核的双核启动功能
# 验证 SMP、GIC、IPI 等多核相关功能

set -e

echo "=========================================="
echo "  Rux 内核 SMP 功能测试"
echo "=========================================="
echo ""

# 检查内核是否已编译
if [ ! -f "target/aarch64-unknown-none/debug/rux" ]; then
    echo "错误: 内核未编译"
    echo "请先运行: make build"
    exit 1
fi

echo "测试 1: 双核启动"
echo "----------------------------------------"
echo "预期输出:"
echo "  ✓ [SMP: Starting CPU boot]"
echo "  ✓ [SMP: PSCI result = 0000000000000000]"
echo "  ✓ [CPU1 up]"
echo "  ✓ [SMP: PSCI success]"
echo "  ✓ SMP: 2 CPUs online"
echo ""

timeout 5 qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a57 \
    -m 2G \
    -smp 2 \
    -nographic \
    -serial mon:stdio \
    -kernel target/aarch64-unknown-none/debug/rux \
    2>&1 | grep -E "(SMP|CPU|online)" || true

echo ""
echo "测试 2: MMU 和 GIC 初始化"
echo "----------------------------------------"
echo "预期输出:"
echo "  ✓ MM: MMU enabled successfully"
echo "  ✓ GIC: Minimal init complete"
echo "  ✓ IRQ enabled"
echo ""

timeout 5 qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a57 \
    -m 2G \
    -smp 2 \
    -nographic \
    -serial mon:stdio \
    -kernel target/aarch64-unknown-none/debug/rux \
    2>&1 | grep -E "(MM:|GIC:|IRQ)" || true

echo ""
echo "测试 3: 系统稳定性"
echo "----------------------------------------"
echo "预期输出:"
echo "  ✓ System ready"
echo "  ✓ Entering main loop"
echo ""

timeout 5 qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a57 \
    -m 2G \
    -smp 2 \
    -nographic \
    -serial mon:stdio \
    -kernel target/aarch64-unknown-none/debug/rux \
    2>&1 | grep -E "(System ready|main loop|Fork)" || true

echo ""
echo "=========================================="
echo "  SMP 测试完成"
echo "=========================================="
echo ""
echo "如果看到以上所有预期输出，说明 SMP 功能正常工作"
echo ""
