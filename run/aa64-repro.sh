#!/usr/bin/env bash
# Repro: run ONE failing LTP binary with visible stdout/stderr on the
# serial console, capture rc. Usage: aa64-repro.sh <test> [kernel] [out]
set -u
T="${1:-getpid01}"
RUN=/home/william/rux-agents/aa64-ltp/run
IMG="$RUN/a64-ltpsmoke.img"
KERNEL="${2:-/home/william/rux-agents/aa64-ltp/target/aarch64-unknown-none/debug/rux}"
OUT="${3:-$RUN/repro-$T-serial.log}"

( sleep 28
  printf 'echo MARK-START; /ltp/%s; echo MARK-RC=$?\n' "$T"
  sleep 25
) | timeout -k 5 95 qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu cortex-a72 -accel tcg,thread=multi \
    -m 1G -smp 2 -nographic -no-reboot \
    -kernel "$KERNEL" \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,drive=rootfs > "$OUT" 2>&1
rc=$?
echo "qemu rc=$rc (124 = timeout, expected)"
echo "--- repro lines ---"
grep -aE "MARK|TPASS|TFAIL|TBROK|TINFO|TCONF|Segmentation|fault|panic" "$OUT" | head -30
echo "--- serial tail ---"
tail -8 "$OUT"
