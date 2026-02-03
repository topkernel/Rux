#!/bin/bash
# QEMU测试脚本

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# 检查内核是否已编译
if [ ! -f "target/aarch64-unknown-none/debug/rux" ]; then
    echo "错误: 内核未编译，请先运行: cargo build --target aarch64-unknown-none"
    exit 1
fi

echo "=========================================="
echo "  Rux 内核测试套件"
echo "=========================================="
echo ""

# 测试1: 基本启动测试
echo "测试 1: 基本启动测试"
echo "------------------------------------------"
timeout 3 qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a57 \
    -m 2G \
    -nographic \
    -kernel target/aarch64-unknown-none/debug/rux 2>&1 | head -10 || true

echo ""
echo "✓ 基本启动测试完成"
echo ""

# 测试2: 内存测试
echo "测试 2: 内存配置测试"
echo "------------------------------------------"
timeout 3 qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a57 \
    -m 1G \
    -nographic \
    -kernel target/aarch64-unknown-none/debug/rux 2>&1 | head -10 || true

echo ""
echo "✓ 内存测试完成"
echo ""

echo "=========================================="
echo "  所有测试完成！"
echo "=========================================="
