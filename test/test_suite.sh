#!/bin/bash
# Rux 内核完整测试套件
# 运行所有测试并生成报告

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# 测试计数器
TOTAL_TESTS=0
PASSED_TESTS=0
FAILED_TESTS=0

# 打印带颜色的消息
print_success() {
    echo -e "${GREEN}✓ $1${NC}"
}

print_error() {
    echo -e "${RED}✗ $1${NC}"
}

print_warning() {
    echo -e "${YELLOW}⚠ $1${NC}"
}

print_header() {
    echo ""
    echo "=========================================="
    echo "  $1"
    echo "=========================================="
    echo ""
}

# 测试函数
run_test() {
    local test_name="$1"
    local test_command="$2"

    TOTAL_TESTS=$((TOTAL_TESTS + 1))
    echo "[$TOTAL_TESTS] 运行: $test_name"

    if eval "$test_command" > /tmp/test_output.txt 2>&1; then
        PASSED_TESTS=$((PASSED_TESTS + 1))
        print_success "$test_name"
        return 0
    else
        FAILED_TESTS=$((FAILED_TESTS + 1))
        print_error "$test_name"
        echo "  错误输出:"
        cat /tmp/test_output.txt | head -5 | sed 's/^/    /'
        return 1
    fi
}

print_header "Rux 内核测试套件"

# 预检查
echo "预检查..."
echo "----------------"

# 检查 Rust
if ! command -v cargo &> /dev/null; then
    print_error "未安装 Rust/Cargo"
    exit 1
fi
print_success "Rust/Cargo"

# 检查 QEMU
if ! command -v qemu-system-aarch64 &> /dev/null; then
    print_warning "未安装 QEMU (部分测试将跳过)"
    QEMU_AVAILABLE=false
else
    QEMU_AVAILABLE=true
    print_success "QEMU"
fi

# 检查构建工具
if command -v rust-objcopy &> /dev/null; then
    print_success "rust-objcopy"
else
    print_warning "未安装 rust-objcopy"
fi

echo ""

# 测试1: 配置文件检查
print_header "配置文件测试"

run_test "Kernel.toml 存在性" "test -f Kernel.toml"
run_test "Cargo.toml 有效性" "cargo check --workspace 2>&1 | grep -q 'Finished'"
run_test "配置文件语法" "grep -q '\\[general\\]' Kernel.toml"

# 测试2: 编译测试
print_header "编译测试"

if $QEMU_AVAILABLE; then
    run_test "Debug 构建" "cargo build --target aarch64-unknown-none"
else
    print_warning "跳过编译测试 (QEMU 不可用)"
fi

# 测试3: 内核二进制测试
print_header "内核二进制测试"

if [ -f "target/aarch64-unknown-none/debug/rux" ]; then
    run_test "ELF 格式检查" "file target/aarch64-unknown-none/debug/rux | grep -q ELF"
    run_test "入口点检查" "readelf -h target/aarch64-unknown-none/debug/rux | grep -q '0x40000000'"
else
    print_warning "内核未编译，跳过二进制测试"
fi

# 测试4: QEMU 启动测试
print_header "QEMU 功能测试"

if $QEMU_AVAILABLE && [ -f "target/aarch64-unknown-none/debug/rux" ]; then
    echo "[$((TOTAL_TESTS + 1))] 运行: QEMU 基本启动测试"
    TOTAL_TESTS=$((TOTAL_TESTS + 1))

    if timeout 3 qemu-system-aarch64 -M virt -cpu cortex-a57 -m 1G -nographic \
        -kernel target/aarch64-unknown-none/debug/rux > /tmp/qemu_output.txt 2>&1; then
        : # QEMU 正常退出
    else
        : # QEMU 超时是正常的 (内核进入主循环)
    fi

    # 检查是否有输出
    if grep -q "Rux" /tmp/qemu_output.txt 2>/dev/null || \
       grep -q "Kernel" /tmp/qemu_output.txt 2>/dev/null; then
        PASSED_TESTS=$((PASSED_TESTS + 1))
        print_success "QEMU 基本启动测试"
        echo "  内核输出:"
        head -3 /tmp/qemu_output.txt | sed 's/^/    /'
    else
        FAILED_TESTS=$((FAILED_TESTS + 1))
        print_error "QEMU 基本启动测试"
        echo "  未检测到内核输出"
    fi
else
    print_warning "跳过 QEMU 测试"
fi

# 测试5: SMP 双核测试
print_header "SMP 功能测试"

if $QEMU_AVAILABLE && [ -f "target/aarch64-unknown-none/debug/rux" ]; then
    echo "[$((TOTAL_TESTS + 1))] 运行: 双核 SMP 启动测试"
    TOTAL_TESTS=$((TOTAL_TESTS + 1))

    if timeout 3 qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -smp 2 -nographic \
        -kernel target/aarch64-unknown-none/debug/rux > /tmp/smp_output.txt 2>&1; then
        :
    fi

    # 检查 SMP 功能
    if grep -aq "SMP.*2 CPUs online" /tmp/smp_output.txt 2>/dev/null || \
       grep -aq "CPU1 up" /tmp/smp_output.txt 2>/dev/null; then
        PASSED_TESTS=$((PASSED_TESTS + 1))
        print_success "双核 SMP 启动测试"
        echo "  SMP 输出:"
        grep -aE "(SMP|CPU|online)" /tmp/smp_output.txt | head -5 | sed 's/^/    /'
    else
        FAILED_TESTS=$((FAILED_TESTS + 1))
        print_error "双核 SMP 启动测试"
        echo "  未检测到 SMP 输出"
    fi
else
    print_warning "跳过 SMP 测试"
fi

# 测试6: MMU 测试
print_header "MMU 功能测试"

if $QEMU_AVAILABLE && [ -f "target/aarch64-unknown-none/debug/rux" ]; then
    echo "[$((TOTAL_TESTS + 1))] 运行: MMU 启用测试"
    TOTAL_TESTS=$((TOTAL_TESTS + 1))

    if timeout 3 qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -smp 1 -nographic \
        -kernel target/aarch64-unknown-none/debug/rux > /tmp/mmu_output.txt 2>&1; then
        :
    fi

    # 检查 MMU 功能
    if grep -aq "MMU enabled successfully" /tmp/mmu_output.txt 2>/dev/null; then
        PASSED_TESTS=$((PASSED_TESTS + 1))
        print_success "MMU 启用测试"
        echo "  MMU 输出:"
        grep -a "MM:" /tmp/mmu_output.txt | head -3 | sed 's/^/    /'
    else
        FAILED_TESTS=$((FAILED_TESTS + 1))
        print_error "MMU 启用测试"
    fi
else
    print_warning "跳过 MMU 测试"
fi

# 测试7: 内存配置测试
print_header "内存配置测试"

if $QEMU_AVAILABLE && [ -f "target/aarch64-unknown-none/debug/rux" ]; then
    echo "[$((TOTAL_TESTS + 1))] 运行: 512MB 内存测试"
    TOTAL_TESTS=$((TOTAL_TESTS + 1))

    if timeout 3 qemu-system-aarch64 -M virt -cpu cortex-a57 -m 512M -smp 1 -nographic \
        -kernel target/aarch64-unknown-none/debug/rux > /tmp/mem512.txt 2>&1; then
        :
    fi

    if [ -s /tmp/mem512.txt ]; then
        PASSED_TESTS=$((PASSED_TESTS + 1))
        print_success "512MB 内存测试"
    else
        FAILED_TESTS=$((FAILED_TESTS + 1))
        print_error "512MB 内存测试"
    fi
else
    print_warning "跳过内存测试"
fi

# 测试总结
print_header "测试总结"

echo "总测试数: $TOTAL_TESTS"
echo -e "${GREEN}通过: $PASSED_TESTS${NC}"
echo -e "${RED}失败: $FAILED_TESTS${NC}"
echo ""

if [ $FAILED_TESTS -eq 0 ]; then
    print_success "所有测试通过！"
    exit 0
else
    print_error "有测试失败"
    exit 1
fi
