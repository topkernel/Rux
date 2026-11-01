#!/bin/bash
# lp-boot4.sh <timeout> <extra-cmdline> — probe boot with monitor + serial socket + gdbstub.
set -u
WT=/home/william/rux-agents/fix-lostpark
TMO="${1:-120}"
EXTRA="${2:-}"
LOG="$WT/run/lp-last3.log"
rm -f "$WT/run/lp-ser.sock" "$WT/run/lp-mon.sock" "$LOG" "$WT/run/lp-qemu.pid"
qemu-system-x86_64 \
    -M q35 -accel kvm -cpu host -m 2G -smp 2 \
    -display none -vga none \
    -chardev socket,id=ser0,path="$WT/run/lp-ser.sock",server=on,wait=off,logfile="$LOG" \
    -serial chardev:ser0 \
    -monitor unix:"$WT/run/lp-mon.sock",server,nowait \
    -gdb tcp::1237 \
    -kernel "$WT/target/x86_64-unknown-none/release/bzImage" \
    -append "root=/dev/vda rw init=/test/semprobe console=ttyS0 $EXTRA" \
    -drive file="$WT/work/probe.img",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
    -device virtio-blk-pci,drive=rootfs \
    -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1 &
QPID=$!
echo "$QPID" > "$WT/run/lp-qemu.pid"
sleep 2
kill -0 "$QPID" 2>/dev/null || { echo "qemu died at start"; exit 1; }
sleep "$((TMO - 2))"
kill -TERM "$QPID" 2>/dev/null
sleep 3
kill -9 "$QPID" 2>/dev/null
echo "boot window over"
