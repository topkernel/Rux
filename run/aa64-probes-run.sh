#!/usr/bin/env bash
# Run the micro-probes in the guest, one boot, visible output.
set -u
RUN=/home/william/rux-agents/aa64-ltp/run
IMG="$RUN/a64-ltpsmoke.img"
KERNEL="${1:-/home/william/rux-agents/aa64-ltp/target/aarch64-unknown-none/debug/rux}"
OUT="$RUN/probe-serial.log"

( sleep 30
  printf '/ltp/aa64probe 1\n'; sleep 5
  printf '/ltp/aa64probe 2\n'; sleep 5
  printf '/ltp/aa64probe 3\n'; sleep 8
  printf '/ltp/aa64probe 4\n'; sleep 5
  printf '/ltp/aa64probe 5\n'; sleep 8
  printf '/ltp/aa64probe 0\n'; sleep 15
) | timeout -k 5 130 qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu cortex-a72 -accel tcg,thread=multi \
    -m 1G -smp 2 -nographic -no-reboot \
    -kernel "$KERNEL" \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,drive=rootfs > "$OUT" 2>&1
rc=$?
echo "qemu rc=$rc"
echo "--- probe lines ---"
grep -aE "P[0-9]-|H-|pagefault|PROBE|Segseg|fault" "$OUT" | grep -v "^\[" | head -60
