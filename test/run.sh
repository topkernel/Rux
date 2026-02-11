#!/bin/bash
# Rux OS 运行脚本
#
# 功能：
# 1. 检查依赖
# 2. 检查内核是否存在，不存在则构建
# 3. 启动 QEMU (带 rootfs)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# 颜色定义
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 打印带颜色的消息
print_header() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}  $1${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo ""
}

print_success() {
    echo -e "${GREEN}✓ $1${NC}"
}

print_error() {
    echo -e "${RED}✗ $1${NC}"
}

print_info() {
    echo -e "${BLUE}ℹ $1${NC}"
}

# 检查依赖
check_dependencies() {
    print_header "检查依赖"

    local missing_deps=0

    # 检查 Rust 工具链
    if ! command -v rustc &> /dev/null; then
        print_error "rustc 未安装"
        missing_deps=$((missing_deps + 1))
    else
        print_success "Rust 工具链: $(rustc --version)"
    fi

    # 检查 Cargo
    if ! command -v cargo &> /dev/null; then
        print_error "cargo 未安装"
        missing_deps=$((missing_deps + 1))
    else
        print_success "Cargo: $(cargo --version)"
    fi

    # 检查 QEMU
    if ! command -v qemu-system-riscv64 &> /dev/null; then
        print_error "qemu-system-riscv64 未安装"
        missing_deps=$((missing_deps + 1))
    else
        print_success "QEMU: $(qemu-system-riscv64 --version | head -1)"
    fi

    # 检查 RISC-V 目标
    if ! rustup target list | grep -q "riscv64gc-unknown-none-elf"; then
        print_info "RISC-V 目标未安装，尝试安装..."
        rustup target add riscv64gc-unknown-none-elf || {
            print_error "安装 RISC-V 目标失败"
            missing_deps=$((missing_deps + 1))
        }
    else
        print_success "RISC-V 目标已安装"
    fi

    if [ $missing_deps -gt 0 ]; then
        print_error "缺少 $missing_deps 个依赖，无法继续"
        exit 1
    fi

    print_success "所有依赖检查通过"
}

# 检查并构建内核
ensure_kernel() {
    print_header "检查内核"

    local KERNEL_BINARY="target/riscv64gc-unknown-none-elf/debug/rux"

    if [ -f "$KERNEL_BINARY" ]; then
        local SIZE=$(ls -lh "$KERNEL_BINARY" | awk '{print $5}')
        print_success "内核已存在: $KERNEL_BINARY ($SIZE)"
    else
        print_info "内核不存在，开始构建..."
        cd "$PROJECT_ROOT"
        cargo build --target riscv64gc-unknown-none-elf --features riscv64

        if [ $? -eq 0 ]; then
            print_success "内核构建成功"
        else
            print_error "内核构建失败"
            exit 1
        fi
    fi
}

# 运行内核
run_kernel() {
    print_header "启动 QEMU"

    print_info "配置: 4核, 2GB 内存, 带 VirtIO-Blk rootfs"
    echo ""

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
    print_header "Rux OS 运行环境"
    echo ""
    echo "启动时间: $(date '+%Y-%m-%d %H:%M:%S')"
    echo ""

    # 检查依赖
    check_dependencies

    # 检查并构建内核
    ensure_kernel

    # 运行内核
    run_kernel
}

# 运行主函数
main "$@"
