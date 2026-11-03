#!/bin/bash
# k10-gdb3.sh — wedge, then read live LAPIC state via APIC MMIO through gdb.
set -u
cd /home/william/rux-agents/ltp-r21
LOG="run/$1"
FIFO=run/k10.fifo
GDBPORT="${2:-1242}"
rm -f "$FIFO" "$LOG"; mkfifo "$FIFO"
(
  sleep 12
  echo "/test/linux-ltp/testcases/bin/kill10 -d 4 -n 10 -g 2 > /tmp/k10.out 2>&1 </dev/null &" > "$FIFO"
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
sleep 42
gdb -q -batch \
  -ex "set pagination off" \
  -ex "target remote :$GDBPORT" \
  -ex "info threads" \
  -ex "x/gx 0xfee000f0" \
  -ex "x/gx 0xfee00320" \
  -ex "x/gx 0xfee00380" \
  -ex "x/gx 0xfee00390" \
  -ex "x/gx 0xfee00100" \
  -ex "x/gx 0xfee00110" \
  -ex "x/gx 0xfee0080" \
  -ex "x/gx 0xfee0090" \
  -ex "x/gx 0xfee00a0" \
  -ex "x/gx 0xfee00b0" \
  -ex "x/gx 0xfee00300" \
  -ex "x/gx 0xfee00350" \
  -ex "detach" \
  > "${LOG%.log}-gdb.txt" 2>&1
sleep 2
kill -9 $QPID 2>/dev/null
kill $FEEDER 2>/dev/null; rm -f "$FIFO"
echo "GDB LAPIC CAPTURE DONE"
