#!/usr/bin/env bash
# aa64-tail task-5 diagnosis: run a few LTP binaries with VISIBLE
# output to classify the smoke failures (assertion vs crash vs loader).
set -u
RUN=/home/william/rux-agents/aa64-tail/run
IMG="$RUN/a64-ltpsmoke.img"
KERNEL="${1:-/home/william/rux-agents/aa64-tail/target/aarch64-unknown-none/debug/rux}"
OUT="${2:-$RUN/ltpdiag-serial.log}"

( sleep 28
  printf 'echo D1; /ltp/getpid01; echo RC1=$?\n'
  sleep 8
  printf 'echo D2; /ltp/write01; echo RC2=$?\n'
  sleep 8
  printf 'echo D3; /ltp/getuid01; echo RC3=$?\n'
  sleep 8
  printf 'echo D4; /ltp/read01; echo RC4=$?\n'
  sleep 10
) | timeout -k 5 110 qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu cortex-a72 -accel tcg,thread=multi \
    -m 1G -smp 2 -nographic -no-reboot \
    -kernel "$KERNEL" \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,drive=rootfs > "$OUT" 2>&1
rc=$?
echo "qemu rc=$rc (124 = timeout, expected)"
echo "--- diagnosis lines ---"
grep -aE "^D[0-9]|RC[0-9]=|TPASS|TFAIL|TBROK|TINFO|not found|Illegal|Segmentation|fault" "$OUT" | head -40
