#!/bin/bash
set -u
cd "$(dirname "$0")/../.." # scripts/test/gnome-diag -> repo root
LOG=/tmp/tq3-console.log
FIFO=/tmp/tq3.fifo
rm -f "$FIFO" "$LOG"; mkfifo "$FIFO"
timeout 220 qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 2G -smp 2 \
  -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file=work/gnome-x86.img,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
  -device virtio-gpu-pci,xres=1280,yres=800 \
  -kernel target/x86_64-unknown-none/debug/bzImage \
  -append "root=/dev/vda rw init=/sbin/runit console=ttyS0 trimq=1" \
  < "$FIFO" > "$LOG" 2>&1 &
Q=$!
exec 3>"$FIFO"
sleep 185
exec 3>&-; wait $Q 2>/dev/null; rm -f "$FIFO"
true
