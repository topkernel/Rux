#!/bin/bash
# SMP 多核测试脚本
# 测试不同核心数配置

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

echo "========================================="
echo "  Rux OS - SMP 多核测试"
echo "========================================="
echo ""

# 测试单核
echo "1. 测试单核模式..."
SMP=1 timeout 3 /home/william/Rux/test/run_riscv64.sh 2>&1 | grep -E "Hart|SMP|test:.*SUCCESS|test: All tests completed" | head -20
echo ""

# 测试双核
echo "2. 测试双核模式..."
SMP=2 timeout 3 /home/william/Rux/test/run_riscv64.sh 2>&1 | grep -E "Hart Count|Boot HART|Hart.*start" | head -10
echo ""

# 测试四核
echo "3. 测试四核模式..."
SMP=4 timeout 3 /home/william/Rux/test/run_riscv64.sh 2>&1 | grep -E "Hart Count|Boot HART|Hart.*start" | head -10
echo ""

echo "========================================="
echo "  SMP 测试完成"
echo "========================================="
