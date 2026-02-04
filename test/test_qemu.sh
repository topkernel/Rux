#!/bin/bash
# QEMU测试脚本
#
# 测试 Rux 内核在不同配置下的运行情况

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# 检查内核是否已编译
if [ ! -f "target/aarch64-unknown-none/debug/rux" ]; then
    echo "错误: 内核未编译，请先运行: make build"
    exit 1
fi

echo "=========================================="
echo "  Rux 内核 QEMU 测试套件"
echo "=========================================="
echo ""

# 测试1: 单核基本启动测试
echo "测试 1: 单核基本启动"
echo "------------------------------------------"
timeout 3 qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a57 \
    -m 2G \
    -smp 1 \
    -nographic \
    -kernel target/aarch64-unknown-none/debug/rux 2>&1 | head -15 || true

echo ""
echo "✓ 单核启动测试完成"
echo ""

# 测试2: 双核SMP测试
echo "测试 2: 双核 SMP 启动"
echo "------------------------------------------"
timeout 3 qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a57 \
    -m 2G \
    -smp 2 \
    -nographic \
    -kernel target/aarch64-unknown-none/debug/rux 2>&1 | grep -E "(SMP|CPU|online)" || true

echo ""
echo "✓ 双核SMP测试完成"
echo ""

# 测试3: 内存配置测试
echo "测试 3: 内存配置测试 (1GB)"
echo "------------------------------------------"
timeout 3 qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a57 \
    -m 1G \
    -smp 2 \
    -nographic \
    -kernel target/aarch64-unknown-none/debug/rux 2>&1 | head -15 || true

echo ""
echo "✓ 内存测试完成"
echo ""

# 测试4: 四核测试
echo "测试 4: 四核配置测试"
echo "------------------------------------------"
timeout 3 qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a57 \
    -m 4G \
    -smp 4 \
    -nographic \
    -kernel target/aarch64-unknown-none/debug/rux 2>&1 | grep -E "(SMP|CPU|online)" || true

echo ""
echo "✓ 四核测试完成"
echo ""

echo "=========================================="
echo "  所有测试完成！"
echo "=========================================="
echo ""
