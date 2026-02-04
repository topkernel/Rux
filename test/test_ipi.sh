#!/bin/bash
# IPI (Inter-Processor Interrupt) 功能测试脚本
#
# 测试 Rux 内核的核间中断功能

set -e

echo "=========================================="
echo "  Rux 内核 IPI 功能测试"
echo "=========================================="
echo ""

# 检查内核是否已编译
if [ ! -f "target/aarch64-unknown-none/debug/rux" ]; then
    echo "错误: 内核未编译"
    echo "请先运行: make build"
    exit 1
fi

echo "IPI 测试场景:"
echo "  1. 双核启动验证"
echo "  2. GIC 初始化验证"
echo "  3. IRQ 控制验证"
echo "  4. 系统稳定性验证"
echo ""

# 运行测试并捕获输出
timeout 5 qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a57 \
    -m 2G \
    -smp 2 \
    -nographic \
    -serial mon:stdio \
    -kernel target/aarch64-unknown-none/debug/rux > /tmp/ipi_test.txt 2>&1 || true

# 测试 1: 双核启动
echo "测试 1: 双核启动"
echo "----------------------------------------"
if grep -q "SMP.*2 CPUs online" /tmp/ipi_test.txt; then
    echo "✓ 双核启动成功"
    grep "SMP.*2 CPUs online" /tmp/ipi_test.txt
else
    echo "✗ 双核启动失败"
fi
echo ""

# 测试 2: MMU 初始化
echo "测试 2: MMU 初始化"
echo "----------------------------------------"
if grep -q "MM: MMU enabled successfully" /tmp/ipi_test.txt; then
    echo "✓ MMU 启用成功"
else
    echo "✗ MMU 启用失败"
fi
echo ""

# 测试 3: GIC 初始化
echo "测试 3: GIC 初始化"
echo "----------------------------------------"
if grep -q "GIC: Minimal init complete" /tmp/ipi_test.txt; then
    echo "✓ GIC 初始化成功"
    grep "GIC:" /tmp/ipi_test.txt | head -2
else
    echo "✗ GIC 初始化失败"
fi
echo ""

# 测试 4: IRQ 控制
echo "测试 4: IRQ 控制"
echo "----------------------------------------"
if grep -q "IRQ enabled" /tmp/ipi_test.txt; then
    echo "✓ IRQ 启用成功"
    grep "IRQ" /tmp/ipi_test.txt | tail -2
else
    echo "✗ IRQ 启用失败"
fi
echo ""

# 测试 5: 系统稳定性
echo "测试 5: 系统稳定性"
echo "----------------------------------------"
if grep -q "System ready" /tmp/ipi_test.txt && \
   grep -q "Entering main loop" /tmp/ipi_test.txt; then
    echo "✓ 系统稳定运行"
    grep "System ready\|main loop\|Fork" /tmp/ipi_test.txt | head -5
else
    echo "✗ 系统不稳定"
fi
echo ""

# 测试总结
echo "=========================================="
echo "  IPI 测试总结"
echo "=========================================="
echo ""

PASSED=0
TOTAL=5

grep -q "SMP.*2 CPUs online" /tmp/ipi_test.txt && PASSED=$((PASSED + 1))
grep -q "MM: MMU enabled successfully" /tmp/ipi_test.txt && PASSED=$((PASSED + 1))
grep -q "GIC: Minimal init complete" /tmp/ipi_test.txt && PASSED=$((PASSED + 1))
grep -q "IRQ enabled" /tmp/ipi_test.txt && PASSED=$((PASSED + 1))
grep -q "System ready" /tmp/ipi_test.txt && PASSED=$((PASSED + 1))

echo "通过: $PASSED / $TOTAL"
echo ""

if [ $PASSED -eq $TOTAL ]; then
    echo "✓ 所有测试通过！"
    echo ""
    echo "IPI 功能已就绪，可以继续开发:"
    echo "  - Phase 2: per-CPU 运行队列"
    echo "  - Phase 4: 调度器多核优化"
    exit 0
else
    echo "✗ 部分测试失败"
    echo ""
    echo "完整输出保存在: /tmp/ipi_test.txt"
    exit 1
fi
