#!/usr/bin/env bash
# GDB hunt: hw-breakpoint the _IO_setb fault site (0x444680) with the
# garbage-fp condition, run /ltp/getpid01 in the guest, capture state.
# Machine boots frozen (-S); gdb attaches, sets bp, continues; bp hits
# when some getpid01-process executes _IO_setb with fp<0x1000.
set -u
RUN=/home/william/rux-agents/aa64-ltp/run
IMG="$RUN/a64-ltpsmoke.img"
KERNEL="${1:-/home/william/rux-agents/aa64-ltp/target/aarch64-unknown-none/debug/rux}"
BIN="$RUN/ltp-aa64/disk-root/ltp/getpid01"
OUT="$RUN/gdb-crash-serial.log"

( sleep 30
  printf '/ltp/getpid01\n'
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
    -ex "hbreak *0x444680 if \$x5 < 0x1000 && \$x5 != 0" \
    -ex "continue" \
    -ex "printf \"=== HIT ===\n\"" \
    -ex "info registers x0 x1 x2 x3 x4 x5 x6 x7 x19 x20 x21 x22 x23 x24 x25 x26 x27 x28 x29 x30 sp pc" \
    -ex "bt 25" \
    -ex "printf \"=== STACK ===\n\"" \
    -ex "x/64gx \$sp" \
    -ex "printf \"=== at fp-0x20 (if mapped) ===\n\"" \
    -ex "x/16gx (\$x5 - 0x20)" \
    -ex "detach" \
    > "$RUN/gdb-crash.log" 2>&1
kill $QPID 2>/dev/null
wait $QPID 2>/dev/null
echo "--- gdb result ---"
head -80 "$RUN/gdb-crash.log"
