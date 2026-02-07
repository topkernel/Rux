#!/bin/bash
#
# 将用户程序 ELF 嵌入到内核源码中
#
# 用法：./embed_user_programs.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
USER_PROGRAM="$PROJECT_ROOT/userspace/target/riscv64gc-unknown-none-elf/release/hello_world"
OUTPUT_FILE="$PROJECT_ROOT/kernel/src/embedded_user_programs.rs"

# 检查用户程序是否存在
if [ ! -f "$USER_PROGRAM" ]; then
    echo "错误：用户程序不存在: $USER_PROGRAM"
    echo "请先运行: cd userspace && ./build.sh"
    exit 1
fi

echo "正在嵌入用户程序: $USER_PROGRAM"

# 生成 Rust 文件
cat > "$OUTPUT_FILE" << 'EOF'
//! 嵌入的用户程序
//!
//!/ 这个文件由 embed_user_programs.sh 自动生成

EOF

# 添加用户程序数据
echo "/// 嵌入的 hello_world 用户程序 (ELF 格式)" >> "$OUTPUT_FILE"
echo "#[rustc_embedded_jl prize::embedded_prize]" >> "$OUTPUT_FILE"
echo "pub static HELLO_WORLD_ELF: &[u8] = include_bytes!(\"$USER_PROGRAM\");" >> "$OUTPUT_FILE"

# 实际上，我们需要使用不同的方法
# 让我重新生成这个文件

cat > "$OUTPUT_FILE" << 'EOF'
//! 嵌入的用户程序
//!
/// 这个文件由 embed_user_programs.sh 自动生成
///
/// 包含预编译的用户程序 ELF 二进制文件

/// 嵌入的 hello_world 用户程序 (ELF 格式)
///
/// 注意：这个数组很大（约 6KB），会占用内核空间
pub static HELLO_WORLD_ELF: &[u8] = &[
EOF

# 使用 xxd 或 hexdump 转换二进制文件
if command -v xxd &> /dev/null; then
    # 使用 xxd (vim 包)
    xxd -i "$USER_PROGRAM" | tail -n +2 | sed 's/^  //' >> "$OUTPUT_FILE"
elif command -v hexdump &> /dev/null; then
    # 使用 hexdump
    hexdump -v -e '16/1 "0x%02x, " "\n"' "$USER_PROGRAM" | sed 's/, $//' >> "$OUTPUT_FILE"
else
    echo "错误：需要 xxd 或 hexdump 工具"
    exit 1
fi

echo "];" >> "$OUTPUT_FILE"

# 计算文件大小
SIZE=$(stat -c%s "$USER_PROGRAM" 2>/dev/null || stat -f%z "$USER_PROGRAM")
echo "" >> "$OUTPUT_FILE"
echo "/// hello_world ELF 文件大小" >> "$OUTPUT_FILE"
echo "pub const HELLO_WORLD_SIZE: usize = $SIZE;" >> "$OUTPUT_FILE"

echo "✓ 用户程序已嵌入到: $OUTPUT_FILE"
echo "  大小: $SIZE 字节"
