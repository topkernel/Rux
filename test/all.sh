#!/bin/bash
# Rux OS 全平台测试套件
#
# 使用方法:
#   ./test/all.sh              # 测试所有平台
#   ./test/all.sh riscv        # 仅测试 RISC-V
#   ./test/all.sh aarch64      # 仅测试 ARM64

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# 颜色定义
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# 测试平台参数
PLATFORM="${1:-all}"

# 打印带颜色的消息
print_header() {
    echo -e "${GREEN}==========================================${NC}"
    echo -e "${GREEN}  $1${NC}"
    echo -e "${GREEN}==========================================${NC}"
    echo ""
}

print_error() {
    echo -e "${RED}[ERROR] $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}[WARN] $1${NC}"
}

# 测试 RISC-V 平台
test_riscv() {
    print_header "RISC-V 64-bit 测试"

    # 检查工具链
    if ! command -v qemu-system-riscv64 &> /dev/null; then
        print_error "qemu-system-riscv64 未安装"
        return 1
    fi

    if [ ! -f "/usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin" ]; then
        print_warning "OpenSBI 未找到，尝试使用默认 BIOS"
        BIOS_FLAG=""
    else
        BIOS_FLAG="-bios /usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin"
    fi

    # 检查内核
    if [ ! -f "target/riscv64gc-unknown-none-elf/debug/rux" ]; then
        echo "内核未编译，正在构建..."
        cargo build --package rux --features riscv64
    fi

    echo "运行 RISC-V 内核测试（10秒超时）..."
    echo ""

    timeout 10 qemu-system-riscv64 \
        -machine virt \
        -cpu rv64 \
        -smp 1 \
        -m 2G \
        -nographic \
        $BIOS_FLAG \
        -kernel target/riscv64gc-unknown-none-elf/debug/rux \
        -serial mon:stdio 2>&1 | head -100

    echo ""
    echo -e "${GREEN}✓ RISC-V 测试完成${NC}"
    echo ""
}

# 测试 ARM64 平台
test_aarch64() {
    print_header "ARM64 (aarch64) 测试"

    # 检查工具链
    if ! command -v qemu-system-aarch64 &> /dev/null; then
        print_error "qemu-system-aarch64 未安装"
        return 1
    fi

    # 检查内核
    if [ ! -f "target/aarch64-unknown-none/debug/rux" ]; then
        echo "内核未编译，正在构建..."
        cargo build --package rux --features aarch64
    fi

    echo "运行 ARM64 内核测试（10秒超时）..."
    echo ""

    timeout 10 qemu-system-aarch64 \
        -M virt,gic-version=3 \
        -cpu cortex-a57 \
        -smp 1 \
        -m 2G \
        -nographic \
        -kernel target/aarch64-unknown-none/debug/rux \
        -serial mon:stdio 2>&1 | head -100

    echo ""
    echo -e "${GREEN}✓ ARM64 测试完成${NC}"
    echo ""
}

# 主测试流程
main() {
    print_header "Rux OS 测试套件"

    case "$PLATFORM" in
        riscv|riscv64)
            test_riscv
            ;;
        aarch64|arm64)
            test_aarch64
            ;;
        all)
            test_riscv
            test_aarch64
            ;;
        *)
            print_error "未知平台: $PLATFORM"
            echo "使用方法: $0 [riscv|aarch64|all]"
            exit 1
            ;;
    esac

    print_header "所有测试完成"
}

main
