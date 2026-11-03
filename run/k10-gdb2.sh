#!/bin/bash
# k10-gdb2.sh — wedge, then gdb: regs + call dump_all_tasks.
set -u
cd /home/william/rux-agents/ltp-r21
LOG="run/$1"
FIFO=run/k10.fifo
GDBPORT="${2:-1241}"
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
sleep 42
gdb -q -batch \
  -ex "set pagination off" \
  -ex "file target/x86_64-unknown-none/debug/rux" \
  -ex "target remote :$GDBPORT" \
  -ex "info registers rip rsp eflags" \
  -ex "thread 2" -ex "info registers rip rsp eflags" \
  -ex "thread 1" \
  -ex "call ((char*(*)(char*))0xffffffff8059a390)(\"gdb-magic\")" \
  -ex "detach" \
  > "${LOG%.log}-gdb.txt" 2>&1
sleep 4
kill -9 $QPID 2>/dev/null
kill $FEEDER 2>/dev/null; rm -f "$FIFO"
echo "GDB CAPTURE DONE"
