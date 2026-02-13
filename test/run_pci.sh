#!/bin/bash
# Rux OS 运行脚本 - PCI VirtIO 测试

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# 检查并构建内核
ensure_kernel() {
    local FEATURES="riscv64"
    local FORCE_REBUILD="${2:-false}"

    if [ "$FORCE_REBUILD" = "true" ] || [ ! -f "target/riscv64gc-unknown-none-elf/debug/rux" ]; then
        echo "构建内核 (特性: $FEATURES)..."
        cargo build --target riscv64gc-unknown-none-elf --features "$FEATURES"
    fi
}

# 运行内核（PCI VirtIO 测试）
run_kernel() {
    echo "启动 QEMU (4核, 2GB 内存, 带 rootfs)..."
    qemu-system-riscv64 \
        -M virt \
        -cpu rv64 \
        -m 2G \
        -nographic \
        -smp 4 \
        -serial mon:stdio \
        -drive file=test/rootfs.img,id=rootfs,format=raw \
        -device virtio-blk-device,disable-legacy=on,drive=rootfs \
        -kernel target/riscv64gc-unknown-none-elf/debug/rux \
        -append "root=/dev/vda rw console=ttyS0 init=/bin/sh"
}

# 主函数
main() {
    local MODE="${1:-run}"

    if [ "$MODE" = "test" ]; then
        # 测试模式
        ensure_kernel "riscv64,unit-test" true
        echo "启动 QEMU (4核, 单元测试)..."
        qemu-system-riscv64 \
            -M virt \
            -cpu rv64 \
            -m 2G \
            -nographic \
            -smp 1 \
            -serial mon:stdio \
            -device virtio-net-device,netdev=user \
            -netdev user,id=user \
            -drive file=test/rootfs.img,if=none,format=raw,id=rootfs \
            -kernel target/riscv64gc-unknown-none-elf/debug/rux
    else
        # 运行模式
        ensure_kernel "riscv64"
        run_kernel
    fi
}
