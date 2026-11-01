#!/bin/bash
# ts-nettest-smp1.sh — lane-local validation of the nettest close-delivery
# leg on a single-CPU boot. The standing nettest-probe boots -smp 2 and is
# currently red ON MAIN (PREEMPT-UNDERFLOW print flood on KTHREAD_MAP,
# preempt-hunt lane); this script isolates MY leg from that wedge.
set -uo pipefail
ROOT="/home/william/rux-agents/test-supplement"
IMG="$ROOT/run/ts-nettest.img"
LOG="$ROOT/run/ts-nettest-serial.log"
BIN="$ROOT/work/leg-nettest"

cp --reflink=auto "$ROOT/run/rootfs-private.img" "$IMG"
printf 'rm /test/nettest\ncd /test\nwrite %s nettest\nsif /test/nettest mode 0100755\n' "$BIN" > "$ROOT/run/ts-inject.cmd"
debugfs -w -f "$ROOT/run/ts-inject.cmd" "$IMG" >/dev/null 2>&1

FIFO="$ROOT/run/ts-nettest.fifo"
rm -f "$FIFO" "$LOG"; mkfifo "$FIFO"
timeout --signal=TERM --kill-after=10 150 \
    qemu-system-riscv64 -M virt -accel tcg,thread=single \
    -cpu rv64,zbb=true,zba=true,zbs=true -m 1G -smp 1 \
    -display none -serial stdio -snapshot \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$ROOT/target/riscv64gc-unknown-none-elf/debug/rux" \
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
    < "$FIFO" > "$LOG" 2>&1 &
QPID=$!
exec 3> "$FIFO"
sleep 40
printf '/test/nettest\n' >&3
sleep 60
exec 3>&-
wait $QPID 2>/dev/null
sed -n 's/\x1b\[[0-9;]*m//g p' "$LOG" | grep -a "udp: echo\|tcp: \|NETTEST\|close-delivery" | tail -8
grep -aq "NETTEST PASS" "$LOG" && echo "TS-NETTEST: PASS" || echo "TS-NETTEST: no pass"
