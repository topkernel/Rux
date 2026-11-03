#!/bin/bash
# tcpdbg.sh — loopback TCP regression repro (riscv64 TCG smp1, nettest probe)
set -uo pipefail
ROOT="/home/william/rux-agents/code-review-perf"
IMG="$ROOT/work/tcpdbg-riscv.img"
LOG="$ROOT/run/tcpdbg-serial.log"
BIN="${BIN:-/home/william/rux-agents/test-supplement/work/leg-nettest}"

[ -x "$BIN" ] || riscv64-linux-gnu-gcc -static -O2 -nostdlib -nostartfiles \
  -march=rv64gc_zicsr -o "$BIN" "$ROOT/test/nettest.c" 2>/dev/null || exit 2
[ -f "$IMG" ] || cp --reflink=auto /home/william/rux-images/testimages/ltp-riscv64-rootfs.img "$IMG"
printf 'rm /test/nettest\ncd /test\nwrite %s nettest\nsif /test/nettest mode 0100755\n' "$BIN" > "$ROOT/run/tcpdbg-inject.cmd"
debugfs -w -f "$ROOT/run/tcpdbg-inject.cmd" "$IMG" >/dev/null 2>&1

FIFO="$ROOT/run/tcpdbg.fifo"
rm -f "$FIFO" "$LOG"; mkfifo "$FIFO"
timeout --signal=TERM --kill-after=10 150 \
    qemu-system-riscv64 -M virt -accel tcg,thread=single \
    -cpu rv64,zbb=true,zba=true,zbs=true -m 1G -smp "${SMP:-1}" \
    -display none -serial stdio -snapshot \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$ROOT/target/riscv64gc-unknown-none-elf/debug/rux" \
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0 ${EXTRA:-}" \
    < "$FIFO" > "$LOG" 2>&1 &
QPID=$!
exec 3> "$FIFO"
sleep ${WAIT:-45}
printf '/test/nettest\n' >&3
sleep ${TAIL:-60}
exec 3>&-
wait $QPID 2>/dev/null
grep -aE "nettest|tcp|TCP" "$LOG" | tail -20
