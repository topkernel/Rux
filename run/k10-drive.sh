#!/bin/bash
# k10-drive.sh — boot LTP rootfs to /bin/sh (PID1) and drive kill10 debug
# via serial stdin. Usage: bash run/k10-drive.sh <logname> <wait1> <wait2> [kill10 args...]
set -u
cd /home/william/rux-agents/ltp-r21
LOG="run/$1"; W1="${2:?}"; W2="${3:?}"; shift 3
ARGS="${*:-}"
FIFO=run/k10.fifo
rm -f "$FIFO" "$LOG"; mkfifo "$FIFO"
(
  sleep 12
  echo "export LTP_IPC_PATH=/tmp/ltp-ipc" > "$FIFO"
  echo "/test/linux-ltp/testcases/bin/kill10 $ARGS > /tmp/k10.out 2>&1 &" > "$FIFO"
  sleep "$W1"
  echo "echo K10DBG-MID; cat /tmp/k10.out; echo K10DBG-MIDEND" > "$FIFO"
  sleep "$W2"
  echo "echo K10DBG-FINAL; cat /tmp/k10.out; echo K10DBG-FINALEND" > "$FIFO"
  sleep 5
  echo "echo K10DBG-DONE" > "$FIFO"
  sleep 2
) &
FEEDER=$!
WT=$((12 + W1 + W2 + 20))
timeout "$WT" qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 2G -smp 2 \
  -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file=test/rootfs-x86_64-ltp.img,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -kernel target/x86_64-unknown-none/debug/bzImage \
  -append "root=/dev/vda rw init=/bin/sh console=ttyS0 dfx=taskdump,watchdog" \
  < "$FIFO" > "$LOG" 2>&1
kill $FEEDER 2>/dev/null; rm -f "$FIFO"
echo "BOOT WINDOW DONE (budget ${WT}s)"
