#!/bin/bash
# r27-probe-drive.sh — boot, cd to a dir, run injected probes in order.
set -u
cd /home/william/rux-agents/ltp-r27
LOG="run/$1"; shift
BUDGET="${@: -1}"; COMMANDS=("${@:1:$#-1}")
FIFO=run/r27p.fifo
rm -f "$FIFO" "$LOG"; mkfifo "$FIFO"
(
  sleep 12
  for c in "${COMMANDS[@]}"; do echo "$c </dev/null" > "$FIFO"; sleep "$BUDGET"; done
  echo "echo R27P-DONE" > "$FIFO"
  sleep 3
) &
FEEDER=$!
timeout $((BUDGET*${#COMMANDS[@]}+40)) qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 2G -smp 2 \
  -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file=test/rootfs-x86_64-ltp.img,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -kernel target/x86_64-unknown-none/debug/bzImage \
  -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
  < "$FIFO" > "$LOG" 2>&1
kill $FEEDER 2>/dev/null; rm -f "$FIFO"
echo "PROBE DONE: $LOG"
