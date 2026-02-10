#!/bin/bash
# SMP 多核启动测试脚本

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

echo "========================================="
echo "Rux OS SMP 多核启动测试"
echo "========================================="
echo ""

# 检查内核是否已构建
KERNEL_BINARY="target/riscv64gc-unknown-none-elf/debug/rux"
if [ ! -f "$KERNEL_BINARY" ]; then
    echo "内核二进制文件不存在，正在构建..."
    cargo build --package rux --features riscv64
fi

# 测试单核启动
echo "测试 1: 单核启动 (SMP=1)"
echo "-----------------------------------"
timeout 2 qemu-system-riscv64 \
    -M virt \
    -cpu rv64 \
    -m 2G \
    -nographic \
    -serial mon:stdio \
    -device virtio-net-device,netdev=user \
    -netdev user,id=user \
    -kernel "$KERNEL_BINARY" \
    -smp 1 2>&1 | grep -E "(HART Count|Boot HART|Hart.*start|SMP initialized)" || true
echo ""

# 测试双核启动
echo "测试 2: 双核启动 (SMP=2)"
echo "-----------------------------------"
timeout 2 qemu-system-riscv64 \
    -M virt \
    -cpu rv64 \
    -m 2G \
    -nographic \
    -serial mon:stdio \
    -device virtio-net-device,netdev=user \
    -netdev user,id=user \
    -kernel "$KERNEL_BINARY" \
    -smp 2 2>&1 | grep -E "(HART Count|Boot HART|Hart.*start|SMP initialized)" || true
echo ""

# 测试四核启动
echo "测试 3: 四核启动 (SMP=4)"
echo "-----------------------------------"
timeout 2 qemu-system-riscv64 \
    -M virt \
    -cpu rv64 \
    -m 2G \
    -nographic \
    -serial mon:stdio \
    -device virtio-net-device,netdev=user \
    -netdev user,id=user \
    -kernel "$KERNEL_BINARY" \
    -smp 4 2>&1 | grep -E "(HART Count|Boot HART|Hart.*start|SMP initialized)" || true
echo ""

echo "========================================="
echo "SMP 测试完成"
echo "========================================="
echo ""
echo "预期结果："
echo "  单核: Hart 0 启动，Hart 1-3 启动失败"
echo "  双核: Hart 0 和 Hart 1 启动，Hart 2-3 启动失败"
echo "  四核: Hart 0-3 全部启动成功"
