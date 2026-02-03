#!/bin/bash
# GDB调试脚本 for Rux Kernel

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

KERNEL_BINARY="target/aarch64-unknown-none/debug/rux"

# 检查内核是否存在
if [ ! -f "$KERNEL_BINARY" ]; then
    echo "错误: 内核未编译，请先运行: cargo build --target aarch64-unknown-none"
    exit 1
fi

# 启动QEMU (暂停，等待GDB连接)
echo "启动 QEMU (调试模式)..."
qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a57 \
    -m 2G \
    -nographic \
    -kernel "$KERNEL_BINARY" \
    -S -s \
    &

QEMU_PID=$!
echo "QEMU PID: $QEMU_PID"

# 等待QEMU启动
sleep 1

# 创建GDB脚本
cat > /tmp/gdb_rux.txt << 'EOF'
# 连接到QEMU
target remote localhost:1234

# 设置架构
set architecture aarch64

# 在入口点设置断点
break *0x40000000

# 继续执行
continue

# 单步执行几条指令
stepi 10
info registers pc x0 x1 sp

# 继续执行
continue

# 等待一段时间
# quit
EOF

# 启动GDB
echo "启动 GDB 调试会话..."
echo "提示: 使用 'quit' 退出 GDB，QEMU 会自动关闭"
gdb-multiarch -q -x /tmp/gdb_rux.txt "$KERNEL_BINARY"

# 清理
kill $QEMU_PID 2>/dev/null || true
rm -f /tmp/gdb_rux.txt

echo "调试会话结束"
