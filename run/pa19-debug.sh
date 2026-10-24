#!/bin/bash
# pa19: riscv64 unit-test main-baseline panic debug boot (ninth leg).
# Same machine shape as test/unittest-boot-riscv.py plus -d int,guest_errors.
# QEMU is killed by this script only (PID captured below).
set -e
cd "$(dirname "$0")/.."
KERNEL=target/riscv64gc-unknown-none-elf/debug/rux
ROOTFS=${RUX_ROOTFS:-/home/william/rux-agents/samgr-callback/test/rootfs.img}
[ -f "$KERNEL" ] || { echo "kernel missing: $KERNEL"; exit 1; }
echo "QEMU PID will be: $$"
timeout 60 qemu-system-riscv64 -M virt -accel tcg,thread=single \
  -cpu rv64,zbb=true,zba=true,zbs=true -m 1G -smp 2 -nographic \
  -drive file=$ROOTFS,if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
  -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
  -kernel $KERNEL \
  -d int,guest_errors -D run/pa19-qemu-int.log < /dev/null > run/pa19-debug-serial.log 2>&1
echo "exit=$?"
