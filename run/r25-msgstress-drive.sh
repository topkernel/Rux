#!/bin/bash
# msgstress01 with LTP_TIMEOUT_MUL — is the timeout throughput or a hang?
set -u
cd /home/william/rux-agents/ltp-r21
LOG="run/$1"
FIFO=run/r25ms.fifo
rm -f "$FIFO" "$LOG"; mkfifo "$FIFO"
(
  sleep 12
  echo "export LTP_TIMEOUT_MUL=3" > "$FIFO"
  echo "cd /test/linux-ltp/testcases/bin && ./msgstress01 </dev/null" > "$FIFO"
  sleep "${2:-660}"
  echo "echo R25MS-DONE" > "$FIFO"
  sleep 3
) &
FEEDER=$!
timeout $((${2:-660}+40)) qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 2G -smp 2 \
  -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file=test/rootfs-x86_64-ltp.img,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -kernel target/x86_64-unknown-none/debug/bzImage \
  -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
  < "$FIFO" > "$LOG" 2>&1
kill $FEEDER 2>/dev/null; rm -f "$FIFO"
echo "R25 MSGSTRESS DRIVE DONE"
