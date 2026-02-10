#!/bin/bash
# GDB 调试脚本 for Rux Kernel (RISC-V)

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

KERNEL_BINARY="target/riscv64gc-unknown-none-elf/debug/rux"

# 检查内核是否存在
if [ ! -f "$KERNEL_BINARY" ]; then
    echo "错误: 内核未编译，请先运行: cargo build --package rux --features riscv64"
    exit 1
fi

# 启动QEMU (暂停，等待GDB连接)
echo "启动 QEMU (调试模式)..."
qemu-system-riscv64 \
    -machine virt \
    -cpu rv64 \
    -m 2G \
    -nographic \
    -bios /usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin \
    -device virtio-net-device,netdev=user \
    -netdev user,id=user \
    -kernel "$KERNEL_BINARY" \
    -S -s \
    &

QEMU_PID=$!
echo "QEMU PID: $QEMU_PID"

# 等待QEMU启动
sleep 1

# 创建GDB脚本
cat > /tmp/gdb_rux_riscv.txt << 'EOF'
# 连接到QEMU
target remote localhost:1234

# 设置架构
set architecture riscv:rv64

# 在 rust_main 设置断点
break rust_main

# 继续执行
continue

# 单步执行几条指令
stepi 5
info registers pc sp ra

# 继续执行
continue

# 等待用户输入
# quit
EOF

# 启动GDB
echo "启动 GDB 调试会话..."
echo "提示: 使用 'quit' 退出 GDB，QEMU 会自动关闭"
riscv64-unknown-elf-gdb -q -x /tmp/gdb_rux_riscv.txt "$KERNEL_BINARY" 2>/dev/null || \
gdb-multiarch -q -x /tmp/gdb_rux_riscv.txt "$KERNEL_BINARY"

# 清理
kill $QEMU_PID 2>/dev/null || true
rm -f /tmp/gdb_rux_riscv.txt

echo "调试会话结束"
