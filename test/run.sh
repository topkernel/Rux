#!/bin/bash
# 运行内核（带 rootfs）

set -e

echo "=== Cleaning up ==="
pkill -9 qemu 2>/dev/null || true
sleep 1

echo "=== Starting kernel with rootfs ==="
echo ""
echo "To view output in real-time, run this in another terminal:"
echo "  cat /tmp/kernel_boot.log"
echo ""

# 使用 unbuffer 来捕获实时输出
if command -v unbuffer &>/dev/null; then
    unbuffer timeout 5 qemu-system-riscv64 \
        -M virt \
        -cpu rv64 \
        -m 2G \
        -nographic \
        -drive file=test/rootfs.img,if=none,format=raw,id=rootfs \
        -device virtio-blk-device,drive=rootfs \
        -kernel target/riscv64gc-unknown-none-elf/debug/rux \
        -append "root=/dev/vda rw init=/bin/sh" > /tmp/kernel_boot.log 2>&1 || true
else
    timeout 5 qemu-system-riscv64 \
        -M virt \
        -cpu rv64 \
        -m 2G \
        -nographic \
        -drive file=test/rootfs.img,if=none,format=raw,id=rootfs \
        -device virtio-blk-device,drive=rootfs \
        -kernel target/riscv64gc-unknown-none-elf/debug/rux \
        -append "root=/dev/vda rw init=/bin/sh" > /tmp/kernel_boot.log 2>&1 || true
fi

echo ""
echo "=== Boot completed ==="
echo "Output saved to: /tmp/kernel_boot.log"
echo ""
echo "Last 50 lines of output:"
tail -50 /tmp/kernel_boot.log || true

# 清理
pkill -9 qemu 2>/dev/null || true
