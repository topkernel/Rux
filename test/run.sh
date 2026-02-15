#!/bin/bash
# Rux OS 运行脚本
#
# 功能：
# 1. 检查内核是否存在，不存在则构建
# 2. 启动 QEMU
#    - test 参数: 使用 unit-test 特性，强制重新编译
#    - run 参数:  不使用 unit-test 特性（控制台模式）
#    - gui 参数:  图形界面模式（启用 VirtIO-GPU 显示）

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

# 运行内核（控制台模式，带 rootfs）
run_kernel() {
    echo "启动 QEMU (4核, 2GB 内存, 控制台模式)..."
    qemu-system-riscv64 \
        -M virt \
        -cpu rv64 \
        -m 2G \
        -smp 4 \
        -nographic \
        -drive file=test/rootfs.img,if=none,id=rootfs,format=raw \
        -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
        -device virtio-gpu-pci \
        -kernel target/riscv64gc-unknown-none-elf/debug/rux \
        -append "root=/dev/vda rw init=/bin/sh"
}

# 运行内核（图形界面模式）
run_kernel_gui() {
    echo "启动 QEMU (4核, 2GB 内存, 图形界面模式)..."
    echo "提示: 在终端 shell 中运行 /bin/desktop 启动桌面"
    qemu-system-riscv64 \
        -M virt \
        -cpu rv64 \
        -m 2G \
        -smp 4 \
        -serial mon:stdio \
        -drive file=test/rootfs.img,if=none,id=rootfs,format=raw \
        -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
        -device virtio-gpu-pci \
        -device qemu-xhci \
        -device usb-kbd \
        -device usb-tablet \
        -kernel target/riscv64gc-unknown-none-elf/debug/rux \
        -append "root=/dev/vda rw init=/bin/sh console=ttyS0"
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
    elif [ "$MODE" = "gui" ]; then
        # 图形界面模式：启用 VirtIO-GPU 显示
        ensure_kernel "riscv64" false
        run_kernel_gui
    else
        # 运行模式：控制台模式
        ensure_kernel "riscv64" false
        run_kernel
    fi
}

# 运行主函数
main "$@"