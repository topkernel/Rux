#!/bin/bash
# k10-gdb.sh — boot wedging kill10 with gdbstub; after wedge, interrupt and dump.
set -u
cd /home/william/rux-agents/ltp-r21
LOG="run/$1"
FIFO=run/k10.fifo
GDBPORT="${2:-1240}"
rm -f "$FIFO" "$LOG"; mkfifo "$FIFO"
(
  sleep 12
  echo "/test/linux-ltp/testcases/bin/kill10 -d 4 -n 10 -g 2 > /dev/shm/k10.out 2>&1 </dev/null &" > "$FIFO"
  sleep 25
) &
FEEDER=$!
timeout 75 qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 2G -smp 2 \
  -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file=test/rootfs-x86_64-ltp.img,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -gdb tcp::$GDBPORT \
  -kernel target/x86_64-unknown-none/debug/bzImage \
  -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
  < "$FIFO" > "$LOG" 2>&1 &
QPID=$!
# wait for the wedge (KILLTRACE x2 then silence): fixed 40s
sleep 42
gdb -q -batch \
  -ex "set pagination off" \
  -ex "file target/x86_64-unknown-none/debug/rux" \
  -ex "target remote :$GDBPORT" \
  -ex "info threads" \
  -ex "thread apply all bt 25" \
  > "${LOG%.log}-gdb.txt" 2>&1
kill -9 $QPID 2>/dev/null
kill $FEEDER 2>/dev/null; rm -f "$FIFO"
echo "GDB CAPTURE DONE"
