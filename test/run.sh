#!/bin/bash
# Rux OS 运行脚本
#
# 功能：
# 1. 检查内核是否存在，不存在则构建
# 2. 启动 QEMU
#    - test 参数: 使用 unit-test 特性，强制重新编译
#    - run 参数:  不使用 unit-test 特性

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# 检查并构建内核
ensure_kernel() {
    local FEATURES="$1"
    local FORCE_REBUILD="${2:-false}"

    if [ "$FORCE_REBUILD" = "true" ] || [ ! -f "target/riscv64gc-unknown-none-elf/debug/rux" ]; then
        echo "构建内核 (特性: $FEATURES)..."
        cargo build --target riscv64gc-unknown-none-elf --features "$FEATURES"
    fi
}

# 运行内核（带 rootfs）
run_kernel() {
    echo "启动 QEMU (4核, 2GB 内存, 带 rootfs)..."
    qemu-system-riscv64 \
        -M virt \
        -cpu rv64 \
        -m 2G \
        -smp 4 \
        -nographic \
        -drive file=test/rootfs.img,if=none,format=raw,id=rootfs \
        -device virtio-blk-device,drive=rootfs \
        -kernel target/riscv64gc-unknown-none-elf/debug/rux \
        -append "root=/dev/vda rw init=/bin/sh"
}

# 主函数
main() {
    local MODE="${1:-run}"

    if [ "$MODE" = "test" ]; then
        # 测试模式：使用 unit-test 特性，强制重新编译
        ensure_kernel "riscv64,unit-test" true
        echo "启动 QEMU (4核, 单元测试)..."
        qemu-system-riscv64 \
            -M virt \
            -cpu rv64 \
            -m 2G \
            -nographic \
            -smp 4 \
            -serial mon:stdio \
            -device virtio-net-device,netdev=user \
            -netdev user,id=user \
            -kernel target/riscv64gc-unknown-none-elf/debug/rux
    else
        # 运行模式：不使用 unit-test 特性
        ensure_kernel "riscv64" false
        run_kernel
    fi
}

# 运行主函数
main "$@"
