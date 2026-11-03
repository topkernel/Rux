#!/bin/bash
# k10-drive3.sh — census shape: stdout+stderr to file, then dump file.
set -u
cd /home/william/rux-agents/ltp-r21
LOG="run/$1"; W1="${2:?}"; shift 2
ARGS="${*:-}"
FIFO=run/k10.fifo
rm -f "$FIFO" "$LOG"; mkfifo "$FIFO"
(
  sleep 12
  echo "/test/linux-ltp/testcases/bin/kill10 $ARGS > /dev/shm/k10.out 2>&1 </dev/null &" > "$FIFO"
  sleep 2
  echo "echo K10DBG-STEP1-ALIVE" > "$FIFO"
  sleep 8
  echo "echo K10DBG-STEP2-ALIVE; cat /dev/shm/k10.out; echo K10DBG-STEP2-END" > "$FIFO"
  sleep "$W1"
  echo "echo K10DBG-STEP3-ALIVE; cat /dev/shm/k10.out; echo K10DBG-STEP3-END" > "$FIFO"
  sleep 4
) &
FEEDER=$!
WT=$((12 + 2 + 8 + W1 + 20))
timeout "$WT" qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 2G -smp 2 \
  -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file=test/rootfs-x86_64-ltp.img,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -kernel target/x86_64-unknown-none/debug/bzImage \
  -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
  < "$FIFO" > "$LOG" 2>&1
kill $FEEDER 2>/dev/null; rm -f "$FIFO"
echo "BOOT WINDOW DONE (budget ${WT}s)"
