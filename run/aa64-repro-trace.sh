#!/usr/bin/env bash
# Repro with SIGTRACE: boot with custom -dtb carrying
# dfx=sigtrace bootargs, run one LTP test, capture SIGTRACE lines.
set -u
T="${1:-getpid01}"
RUN=/home/william/rux-agents/aa64-ltp/run
IMG="$RUN/a64-ltpsmoke.img"
KERNEL="${2:-/home/william/rux-agents/aa64-ltp/target/aarch64-unknown-none/debug/rux}"
OUT="$RUN/trace-$T-serial.log"

( sleep 30
  printf '/ltp/%s\n' "$T"
  sleep 60
) | timeout -k 5 125 qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu cortex-a72 -accel tcg,thread=multi \
    -m 1G -smp 2 -nographic -no-reboot \
    -kernel "$KERNEL" \
    -dtb "$RUN/qemu-virt.dtb" \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,drive=rootfs > "$OUT" 2>&1
rc=$?
echo "qemu rc=$rc"
echo "--- cmd line seen by kernel ---"
grep -a "cmd:" "$OUT" | head -2
echo "--- SIGTRACE lines ---"
grep -a "SIGTRACE" "$OUT" | head -60
echo "--- crash ---"
grep -aE "pagefault|Timeout per run|TPASS" "$OUT" | head -6
