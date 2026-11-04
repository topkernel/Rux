#!/bin/bash
# r25-drive.sh — run one LTP case console-direct for full output.
# Usage: run/r25-drive.sh <logname> <testcase-path> [budget_s] [extra-append]
set -u
cd /home/william/rux-agents/ltp-r21
LOG="run/$1"
CASE="$2"
BUDGET="${3:-45}"
APPEND="${4:-}"
FIFO=run/r25.fifo
rm -f "$FIFO" "$LOG"; mkfifo "$FIFO"
(
  sleep 12
  echo "$CASE </dev/null" > "$FIFO"
  sleep "$BUDGET"
  echo "echo R25DBG-DONE" > "$FIFO"
  sleep 3
) &
FEEDER=$!
timeout $((BUDGET+30)) qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 2G -smp 2 \
  -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file=test/rootfs-x86_64-ltp.img,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -kernel target/x86_64-unknown-none/debug/bzImage \
  -append "root=/dev/vda rw init=/bin/sh console=ttyS0 $APPEND" \
  < "$FIFO" > "$LOG" 2>&1
kill $FEEDER 2>/dev/null; rm -f "$FIFO"
echo "R25 DRIVE DONE: $LOG"
