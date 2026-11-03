#!/bin/bash
# k10-drive4.sh — wedge then DUMP! magic taskdump.
set -u
cd /home/william/rux-agents/ltp-r21
LOG="run/$1"
FIFO=run/k10.fifo
rm -f "$FIFO" "$LOG"; mkfifo "$FIFO"
(
  sleep 12
  echo "/test/linux-ltp/testcases/bin/kill10 -d 4 -n 10 -g 2 > /tmp/k10.out 2>&1 </dev/null &" > "$FIFO"
  sleep 15
  printf 'DUMP!\n' > "$FIFO"
  sleep 8
  printf 'DUMP!\n' > "$FIFO"
  sleep 5
) &
FEEDER=$!
timeout 55 qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 2G -smp 2 \
  -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file=test/rootfs-x86_64-ltp.img,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -kernel target/x86_64-unknown-none/debug/bzImage \
  -append "root=/dev/vda rw init=/bin/sh console=ttyS0 dfx=taskdump" \
  < "$FIFO" > "$LOG" 2>&1
kill $FEEDER 2>/dev/null; rm -f "$FIFO"
echo "BOOT WINDOW DONE"
