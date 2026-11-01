#!/bin/bash
# probe-boot.sh <probename> — boot the injected image once, dump serial.
set -eu
cd /home/william/rux-agents/review-compat
NAME="${1:?probe name}"
IMG=run/rootfs-ltp-compat.img
KERN=target/riscv64gc-unknown-none-elf/debug/rux
LOG=run/probe-serial-$NAME.log
timeout 300 qemu-system-riscv64 -M virt -accel tcg,thread=single \
  -cpu rv64,zbb=true,zba=true,zbs=true -m 2G -smp 2 -nographic -snapshot \
  -serial mon:stdio \
  -drive file=$IMG,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
  -kernel $KERN -append "root=/dev/vda rw init=/test/ltpscan console=ttyS0" \
  > "$LOG" 2>&1 || true
echo "--- serial result lines ---"
grep -a 'RES|' "$LOG" || tail -5 "$LOG"
