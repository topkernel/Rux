#!/usr/bin/env bash
# GDB hunt #2: hw-bp at _IO_setb ENTRY (0x444620) with fp<0x1000 cond,
# capture lr (caller PC) and args. Also dump page-0 mapping via /proc
# in-guest later. Usage: aa64-gdb-entry.sh [test-binary-name]
set -u
RUN=/home/william/rux-agents/aa64-ltp/run
IMG="$RUN/a64-ltpsmoke.img"
KERNEL="${1:-/home/william/rux-agents/aa64-ltp/target/aarch64-unknown-none/debug/rux}"
T="${2:-getpid01}"
BIN="$RUN/ltp-aa64/disk-root/ltp/$T"
OUT="$RUN/gdb-entry-serial.log"

ENTRY=$(aarch64-linux-gnu-nm "$BIN" | awk '$3=="_IO_setb"{print $0}' | head -1)
echo "symbol: $ENTRY"

( sleep 30
  printf '/ltp/%s\n' "$T"
  sleep 170
) | timeout -k 5 210 qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu cortex-a72 -accel tcg,thread=multi \
    -m 1G -smp 2 -nographic -no-reboot \
    -kernel "$KERNEL" -s -S \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
    -device virtio-blk-pci,drive=rootfs > "$OUT" 2>&1 &
QPID=$!

sleep 2
gdb-multiarch -batch \
    -ex "set pagination off" \
    -ex "set confirm off" \
    -ex "file $BIN" \
    -ex "target remote :1234" \
    -ex "hbreak *0x444620 if \$x0 < 0x1000 && \$x0 != 0" \
    -ex "continue" \
    -ex "printf \"=== HIT _IO_setb ENTRY ===\n\"" \
    -ex "info registers x0 x1 x2 x3 x19 x20 x21 x22 x23 x24 x25 x26 x27 x28 x29 x30 sp pc" \
    -ex "printf \"=== caller disasm (lr-0x40) ===\n\"" \
    -ex "x/24i (\$x30 - 0x40)" \
    -ex "printf \"=== caller stack ===\n\"" \
    -ex "x/48gx \$sp" \
    -ex "detach" \
    > "$RUN/gdb-entry.log" 2>&1
kill $QPID 2>/dev/null
wait $QPID 2>/dev/null
echo "--- gdb result ---"
head -100 "$RUN/gdb-entry.log"
