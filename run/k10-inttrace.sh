#!/bin/bash
# k10-inttrace.sh — wedge boot with -d int interrupt tracing.
set -u
cd /home/william/rux-agents/ltp-r21
LOG="run/$1"
FIFO=run/k10.fifo
INTLOG="run/$1.inttrace"
rm -f "$FIFO" "$LOG" "$INTLOG"; mkfifo "$FIFO"
(
  sleep 12
  echo "/test/linux-ltp/testcases/bin/kill10 -d 4 -n 10 -g 2 > /tmp/k10.out 2>&1 </dev/null &" > "$FIFO"
  sleep 90
) &
FEEDER=$!
timeout 120 qemu-system-x86_64 -M q35 -accel tcg,thread=single -cpu max -m 2G -smp 2 \
  -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file=test/rootfs-x86_64-ltp.img,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -d int -D "$INTLOG" \
  -kernel target/x86_64-unknown-none/debug/bzImage \
  -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
  < "$FIFO" > "$LOG" 2>&1
kill $FEEDER 2>/dev/null; rm -f "$FIFO"
echo "INTTRACE BOOT DONE: $(wc -l < "$INTLOG") trace lines"
