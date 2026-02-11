#!/bin/bash
#
# 将用户程序 ELF 嵌入到内核源码中
#
# 用法：./embed_user_programs.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
USERSPACE_DIR="$PROJECT_ROOT/userspace"
OUTPUT_FILE="$PROJECT_ROOT/kernel/src/embedded_user_programs.rs"

# 嵌入的用户程序列表
PROGRAMS="shell hello_world"

echo "正在嵌入用户程序..."

# 生成 Rust 文件
cat > "$OUTPUT_FILE" << 'EOF'
//! 嵌入的用户程序
//!
/// 这个文件由 embed_user_programs.sh 自动生成
///
/// 包含预编译的用户程序 ELF 二进制文件

EOF

# 嵌入每个程序
for prog in $PROGRAMS; do
    PROG_FILE="$USERSPACE_DIR/target/riscv64gc-unknown-none-elf/release/$prog"

    # 转换为大写
    PROG_UPPER=$(echo "$prog" | tr '[:lower:]' '[:upper:]')

    if [ ! -f "$PROG_FILE" ]; then
        echo "警告：跳过不存在的程序: $prog"
        continue
    fi

    echo "嵌入: $prog"

    # 添加注释
    echo "/// 嵌入的 $prog 用户程序 (ELF 格式)" >> "$OUTPUT_FILE"
    echo "pub static ${PROG_UPPER}_ELF: &[u8] = &[" >> "$OUTPUT_FILE"

    # 使用 xxd 或 hexdump 转换二进制文件
    if command -v xxd &> /dev/null; then
        xxd -i "$PROG_FILE" | grep -v "^unsigned int" | tail -n +2 | sed 's/^  //' | sed 's/^};$/];/' >> "$OUTPUT_FILE"
    elif command -v hexdump &> /dev/null; then
        hexdump -v -e '16/1 "0x%02x, " "\n"' "$PROG_FILE" | sed 's/, $//' >> "$OUTPUT_FILE"
        echo "];" >> "$OUTPUT_FILE"
    else
        echo "错误：需要 xxd 或 hexdump 工具"
        exit 1
    fi

    # 计算文件大小
    SIZE=$(stat -c%s "$PROG_FILE" 2>/dev/null || stat -f%z "$PROG_FILE")
    echo "" >> "$OUTPUT_FILE"
    echo "/// $prog ELF 文件大小" >> "$OUTPUT_FILE"
    echo "pub const ${PROG_UPPER}_SIZE: usize = $SIZE;" >> "$OUTPUT_FILE"
    echo "" >> "$OUTPUT_FILE"

    echo "  ✓ $prog ($SIZE 字节)"
done

echo "✓ 用户程序已嵌入到: $OUTPUT_FILE"

