#!/bin/bash
# Rux OS 全量单元测试脚本
#
# 功能：
# 1. 构建内核（带 unit-test 特性）
# 2. 启动 QEMU
# 3. 运行所有单元测试
# 4. 收集和显示测试结果

set -e  # 遇到错误立即退出

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

print_warning() {
    echo -e "${YELLOW}⚠ $1${NC}"
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
        print_success "QEMU: $(qemu-system-riscv64 --version)"
    fi

    # 检查 RISC-V 目标
    if ! rustup target list | grep -q "riscv64gc-unknown-none-elf"; then
        print_warning "RISC-V 目标未安装，尝试安装..."
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

# 构建内核
build_kernel() {
    print_header "构建内核（带单元测试）"

    local BUILD_MODE="${BUILD_MODE:-debug}"
    local FEATURES="riscv64,unit-test"

    print_info "构建模式: $BUILD_MODE"
    print_info "特性: $FEATURES"

    # 构建内核
    cargo build --package rux --features $FEATURES

    if [ $? -eq 0 ]; then
        print_success "内核构建成功"
    else
        print_error "内核构建失败"
        exit 1
    fi

    # 显示二进制文件信息
    local KERNEL_BINARY="target/riscv64gc-unknown-none-elf/$BUILD_MODE/rux"
    if [ -f "$KERNEL_BINARY" ]; then
        local SIZE=$(ls -lh "$KERNEL_BINARY" | awk '{print $5}')
        print_success "内核二进制: $KERNEL_BINARY ($SIZE)"
    else
        print_error "内核二进制文件不存在"
        exit 1
    fi
}

# 运行单元测试
run_unit_tests() {
    print_header "运行单元测试"

    local BUILD_MODE="${BUILD_MODE:-debug}"
    local KERNEL_BINARY="target/riscv64gc-unknown-none-elf/$BUILD_MODE/rux"
    local TIMEOUT="${TEST_TIMEOUT:-10}"

    print_info "超时时间: ${TIMEOUT}秒"
    print_info "启动 QEMU..."
    echo ""

    # 运行 QEMU 并捕获输出
    timeout $TIMEOUT qemu-system-riscv64 \
        -M virt \
        -cpu rv64 \
        -m 2G \
        -nographic \
        -serial mon:stdio \
        -device virtio-net-device,netdev=user \
        -netdev user,id=user \
        -kernel "$KERNEL_BINARY" \
        -smp 1 2>&1 | tee /tmp/rux_test_output.$$.txt

    local QEMU_EXIT_CODE=${PIPESTATUS[0]}

    echo ""
    print_header "测试结果分析"

    # 分析测试输出
    local output_file="/tmp/rux_test_output.$$.txt"

    # 统计测试模块
    local total_tests=$(grep -c "^test: [0-9]*\\. " "$output_file" 2>/dev/null || echo "0")
    local passed_tests=$(grep -c "SUCCESS" "$output_file" 2>/dev/null || echo "0")
    local failed_tests=$(grep -c "FAILED\|PANIC" "$output_file" 2>/dev/null || echo "0")

    # 检查测试完成标记
    if grep -q "test: ===== All Unit Tests Completed =====" "$output_file"; then
        print_success "所有单元测试已完成"
    else
        print_warning "测试未完成或被超时终止"
    fi

    # 显示统计信息
    echo ""
    echo -e "${BLUE}测试统计：${NC}"
    echo "  总测试项: $total_tests"
    echo -e "  ${GREEN}通过: $passed_tests${NC}"

    if [ "$failed_tests" -gt 0 ]; then
        echo -e "  ${RED}失败: $failed_tests${NC}"
        echo ""
        echo "失败的测试："
        grep -E "FAILED|PANIC" "$output_file" | head -10
    else
        echo -e "  ${GREEN}失败: 0${NC}"
    fi

    # 清理临时文件
    rm -f "$output_file"

    # 判断测试是否成功
    if [ "$failed_tests" -eq 0 ] && grep -q "All Unit Tests Completed" "$output_file" 2>/dev/null; then
        return 0
    else
        return 1
    fi
}

# 主函数
main() {
    print_header "Rux OS 全量单元测试"
    echo ""
    echo "测试时间: $(date '+%Y-%m-%d %H:%M:%S')"
    echo ""

    # 检查依赖
    check_dependencies

    # 构建内核
    build_kernel

    # 运行测试
    if run_unit_tests; then
        echo ""
        print_header "测试结果"
        print_success "所有单元测试通过！"
        exit 0
    else
        echo ""
        print_header "测试结果"
        print_error "部分测试失败，请检查输出"
        exit 1
    fi
}

# 运行主函数
main "$@"
