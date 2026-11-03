#!/bin/bash
# ft-drive.sh — run ftest03 console-direct for full child output.
set -u
cd /home/william/rux-agents/ltp-r21
LOG="run/$1"
FIFO=run/k10.fifo
rm -f "$FIFO" "$LOG"; mkfifo "$FIFO"
(
  sleep 12
  echo "/test/linux-ltp/testcases/bin/ftest03 </dev/null" > "$FIFO"
  sleep 30
  echo "echo FTDBG-DONE" > "$FIFO"
  sleep 3
) &
FEEDER=$!
timeout 60 qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 2G -smp 2 \
  -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file=test/rootfs-x86_64-ltp.img,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -kernel target/x86_64-unknown-none/debug/bzImage \
  -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
  < "$FIFO" > "$LOG" 2>&1
kill $FEEDER 2>/dev/null; rm -f "$FIFO"
echo "FT DRIVE DONE"
