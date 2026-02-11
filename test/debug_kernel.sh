#!/bin/bash
#
# 使用 GDB 调试内核启动

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

KERNEL_BINARY="target/riscv64gc-unknown-none-elf/debug/rux"

echo "Starting QEMU with GDB server on localhost:1234..."
echo "Connect with: riscv64-unknown-elf-gdb $KERNEL_BINARY"
echo ""
echo "GDB commands:"
echo "  (gdb) target remote localhost:1234"
echo "  (gdb) break _start"
echo "  (gdb) continue"
echo ""
echo "Press Ctrl+C to stop QEMU"

qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
  -serial mon:stdio \
  -bios none \
  -kernel "$KERNEL_BINARY" \
  -s -S
