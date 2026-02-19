#!/bin/bash
# Rux OS 运行脚本
#
# 功能：
# 1. 检查内核是否存在，不存在则构建
# 2. 启动 QEMU
#    - test 参数: 使用 unit-test 特性，强制重新编译
#    - console 参数:  控制台模式（可指定 init 程序）
#    - gui 参数:  图形界面模式（启用 VirtIO-GPU 显示）
#
# 用法:
#   ./run.sh [mode] [init]
#   mode: console | gui | test
#   init: /bin/shell | /bin/cshell | /bin/rust-shell

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# 默认 init 程序
DEFAULT_INIT="/bin/shell"

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
    local INIT="${1:-$DEFAULT_INIT}"
    echo "启动 QEMU (4核, 2GB 内存, 控制台模式, init=$INIT)..."

    # 检查是否在 WSL 中运行
    if grep -qi microsoft /proc/version 2>/dev/null; then
        echo "检测到 WSL 环境，使用特殊配置..."
        # WSL: 使用 chardev 方式，可能更好地处理终端输入
        qemu-system-riscv64 \
            -M virt \
            -cpu rv64 \
            -m 2G \
            -smp 4 \
            -nographic \
            -chardev stdio,id=char0,mux=on \
            -serial chardev:char0 \
            -mon chardev=char0 \
            -drive file=test/rootfs.img,if=none,id=rootfs,format=raw \
            -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
            -device virtio-gpu-pci \
            -kernel target/riscv64gc-unknown-none-elf/debug/rux \
            -append "root=/dev/vda rw init=$INIT console=ttyS0"
    else
        # 非 WSL: 使用标准配置
        qemu-system-riscv64 \
            -M virt \
            -cpu rv64 \
            -m 2G \
            -smp 4 \
            -nographic \
            -serial mon:stdio \
            -drive file=test/rootfs.img,if=none,id=rootfs,format=raw \
            -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
            -device virtio-gpu-pci \
            -kernel target/riscv64gc-unknown-none-elf/debug/rux \
            -append "root=/dev/vda rw init=$INIT console=ttyS0"
    fi
}

# 运行内核（图形界面模式）
run_kernel_gui() {
    local INIT="${1:-$DEFAULT_INIT}"
    echo "启动 QEMU (4核, 2GB 内存, 图形界面模式, init=$INIT)..."
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
        -append "root=/dev/vda rw init=$INIT console=ttyS0"
}

# 主函数
main() {
    local MODE="${1:-console}"
    local INIT="${2:-$DEFAULT_INIT}"

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
        run_kernel_gui "$INIT"
    else
        # 控制台模式
        ensure_kernel "riscv64" false
        run_kernel "$INIT"
    fi
}

# 运行主函数
main "$@"