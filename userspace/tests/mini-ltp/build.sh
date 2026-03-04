#!/bin/bash
# Rux OS - Mini LTP 测试套件构建脚本
#
# 最小化的内核兼容性测试集，测试核心系统调用

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUTPUT_DIR="${SCRIPT_DIR}/output"
MUSL_DIR="${PROJECT_ROOT}/toolchain/riscv64-rux-linux-musl"

echo "========================================"
echo "Rux OS - Mini LTP Build Script"
echo "========================================"
echo "OUTPUT_DIR: ${OUTPUT_DIR}"
echo "MUSL_DIR: ${MUSL_DIR}"
echo ""

# 检查交叉编译工具链
if ! command -v riscv64-linux-gnu-gcc &> /dev/null; then
    echo "Error: riscv64-linux-gnu-gcc not found"
    exit 1
fi

# 检查 musl 目录
if [ ! -d "$MUSL_DIR/include" ]; then
    echo "Error: musl include directory not found"
    exit 1
fi

# 创建输出目录
mkdir -p "$OUTPUT_DIR/bin"

# 编译器设置
CC=riscv64-linux-gnu-gcc
CFLAGS="-static -O2 -nostdinc -isystem ${MUSL_DIR}/include -isystem /usr/riscv64-linux-gnu/include"
# 注意：链接顺序很重要！crt1.o, crti.o 在前，然后是代码，然后是 -lc -lgcc，最后是 crtn.o

echo "Building test programs..."
echo ""

BUILD_OK=0
BUILD_FAIL=0

# 编译所有测试程序
for src in "$SCRIPT_DIR"/src/*.c; do
    if [ -f "$src" ]; then
        name=$(basename "$src" .c)
        echo -n "  Building: $name... "
        # 分步编译：先编译成 .o，再链接
        if $CC $CFLAGS -c -o "$OUTPUT_DIR/${name}.o" "$src" 2>/dev/null && \
           $CC -static -nostdlib \
               -L${MUSL_DIR}/lib \
               ${MUSL_DIR}/lib/crt1.o \
               ${MUSL_DIR}/lib/crti.o \
               "$OUTPUT_DIR/${name}.o" \
               -lc -lgcc \
               ${MUSL_DIR}/lib/crtn.o \
               -o "$OUTPUT_DIR/bin/$name" 2>/dev/null; then
            rm -f "$OUTPUT_DIR/${name}.o"
            riscv64-linux-gnu-strip "$OUTPUT_DIR/bin/$name" 2>/dev/null
            echo "OK"
            BUILD_OK=$((BUILD_OK + 1))
        else
            rm -f "$OUTPUT_DIR/${name}.o"
            echo "FAILED"
            BUILD_FAIL=$((BUILD_FAIL + 1))
        fi
    fi
done

echo ""
echo "Build complete: $BUILD_OK OK, $BUILD_FAIL failed"

# 创建测试运行脚本
cat > "$OUTPUT_DIR/run_tests.sh" << 'EOF'
#!/bin/sh
# Mini LTP 测试运行脚本

TEST_DIR=/test/mini-ltp/bin
PASSED=0
FAILED=0
SKIPPED=0

echo "========================================"
echo "Rux OS Kernel Compatibility Tests"
echo "========================================"
echo ""

run_test() {
    test_name=$1
    if [ -x "$TEST_DIR/$test_name" ]; then
        echo -n "Testing $test_name... "
        if "$TEST_DIR/$test_name" > /dev/null 2>&1; then
            echo "PASS"
            PASSED=$((PASSED + 1))
        else
            echo "FAIL"
            FAILED=$((FAILED + 1))
        fi
    else
        SKIPPED=$((SKIPPED + 1))
    fi
}

# 运行所有测试
for test in "$TEST_DIR"/*; do
    if [ -x "$test" ]; then
        name=$(basename "$test")
        run_test "$name"
    fi
done

echo ""
echo "========================================"
echo "Results: $PASSED passed, $FAILED failed, $SKIPPED skipped"
echo "========================================"

if [ $FAILED -eq 0 ]; then
    exit 0
else
    exit 1
fi
EOF
chmod +x "$OUTPUT_DIR/run_tests.sh"

# 显示结果
echo ""
echo "========================================"
echo "Build Summary"
echo "========================================"
TEST_COUNT=$(find "$OUTPUT_DIR/bin" -type f -executable 2>/dev/null | wc -l)
echo "Test binaries built: $TEST_COUNT"
echo "Output directory: $OUTPUT_DIR"
du -sh "$OUTPUT_DIR"
echo ""
ls -la "$OUTPUT_DIR/bin/"
echo ""
echo "Build completed!"
