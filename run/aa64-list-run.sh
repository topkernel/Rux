#!/usr/bin/env bash
# Run an explicit list of LTP tests in one boot, plain serial commands
# (no shell functions — the serial-fed toybox sh dropped function defs).
# Usage: aa64-list-run.sh "t1 t2 ..." [kernel] [out]
set -u
LIST="${1:?test list}"
RUN=/home/william/rux-agents/aa64-ltp/run
IMG="$RUN/a64-ltpsmoke.img"
KERNEL="${2:-/home/william/rux-agents/aa64-ltp/target/aarch64-unknown-none/debug/rux}"
OUT="${3:-$RUN/listrun-serial.log}"

FEED="$RUN/listrun-feed.sh"
{
  echo 'sleep 35'
  echo 'printf "export LTP_TIMEOUT_MUL=4\\n"'
  for t in $LIST; do
    echo "printf 'timeout 150 /ltp/$t >/dev/null 2>&1; echo LTPSMK-$t-RC=\$?\\n'"
    echo "sleep 8"
  done
  echo 'sleep 10'
} > "$FEED"
bash "$FEED" | timeout -k 5 1500 qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu cortex-a72 -accel tcg,thread=multi \
    -m 1G -smp 2 -nographic -no-reboot \
    -kernel "$KERNEL" \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,drive=rootfs > "$OUT" 2>&1
echo "qemu done"
grep -ao "LTPSMK-[a-z0-9]*-RC=[0-9]*" "$OUT" | sort -u
