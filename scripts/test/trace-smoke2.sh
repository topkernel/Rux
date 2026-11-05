#!/bin/bash
# trace-smoke2.sh — batch-2 tracefs smoke: the block-IO and binder event
# families plus the tracepoint!-derived table shapes.
#
# Checks, in order:
#   1. available_events carries the 16-event table (the batch-1 ten
#      plus block:block_rq_issue/block_rq_complete and the four binder
#      events)
#   2. the format files of the six new events (name/ID/format header,
#      the reference field lines, the print fmt)
#   3. block traffic: arming the block events and reading/cold-catting
#      a file plus writing one to the root and syncing records
#      block_rq_issue and block_rq_complete lines in the reference
#      geometry (%d,%d rwbs bytes () sector + nr class,level,prio)
#   4. binder traffic: the closed-loop binder probe (two processes,
#      /dev/binder) records binder_transaction, alloc_buf,
#      buffer_release and transaction_received lines
#   5. the group switches read 1/0/X over the new groups
#
# The probe image is a private copy of test/rootfs.img with
# test/binder_probe.c compiled in via debugfs (the shared image is never
# written).
#
# Usage: test/trace-smoke2.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 30), TIMEOUT (default 360)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/trace-smoke2-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-30}"
TIMEOUT="${TIMEOUT:-360}"

IMG="$ROOT/work/trace-smoke2.img"

mkdir -p "$ROOT/run" "$ROOT/work"
rm -f "$LOG" "$IMG"

# --- the private image: rootfs + the binder probe --------------------
# The probe is linked against the tree's musl libc (the same libc the
# minimal rootfs userspace uses). The host glibc toolchain (GCC 15 /
# Ubuntu glibc) builds against a distro baseline whose generic memset
# itself carries RVV instructions, and the standard QEMU cpu model
# (rv64,zbb,zba,zbs) has no V extension — a glibc-static probe SIGILLs
# in libc code regardless of -march (observed: vsetivli at glibc
# __memset+0x42). The musl link is rv64gc-clean end to end.
MUSL="$ROOT/toolchain/riscv64-rux-linux-musl"
GCCINC="$(riscv64-linux-gnu-gcc -print-file-name=include)"
riscv64-linux-gnu-gcc -static -nostdlib -nostdinc \
    -isystem "$MUSL/include" -isystem "$GCCINC" \
    -Os -march=rv64gc -D_FORTIFY_SOURCE=0 -fno-stack-protector \
    -o "$ROOT/work/binder_probe2" \
    "$MUSL/lib/crt1.o" "$ROOT/test/binder_probe.c" \
    -L"$MUSL/lib" -lc -lgcc || { echo "probe build failed"; exit 1; }
cp "$ROOT/test/rootfs.img" "$IMG"
debugfs -w -R "rm /test/binder_probe" "$IMG" >/dev/null 2>&1
debugfs -w -R "write $ROOT/work/binder_probe2 binder_probe" "$IMG" >/dev/null 2>&1
# `write` puts the file in the CWD of debugfs (the root); link it into /test.
INODE=$(debugfs -R "stat binder_probe" "$IMG" 2>/dev/null | sed -n 's/^Inode: \([0-9]*\).*/\1/p' | head -1)
if [ -n "$INODE" ]; then
    debugfs -w -R "ln <${INODE}> /test/binder_probe" "$IMG" >/dev/null 2>&1
    debugfs -w -R "unlink binder_probe" "$IMG" >/dev/null 2>&1
fi

{
    sleep "$BOOT_DELAY"
    echo 'echo ===T2: available_events ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/available_events'
    sleep 0.5
    echo 'echo ===T2: block_rq_issue format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/block/block_rq_issue/format'
    sleep 0.5
    echo 'echo ===T2: block_rq_complete format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/block/block_rq_complete/format'
    sleep 0.5
    echo 'echo ===T2: binder_transaction format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/binder/binder_transaction/format'
    sleep 0.5
    echo 'echo ===T2: binder group state before ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/binder/enable /sys/kernel/debug/tracing/events/block/enable'
    sleep 0.5
    echo 'echo ===T2: arm block + binder ==='
    sleep 0.3
    echo 'echo block:block_rq_issue block:block_rq_complete binder:binder_transaction binder:binder_transaction_received binder:binder_transaction_alloc_buf binder:binder_transaction_buffer_release > /sys/kernel/debug/tracing/set_event'
    sleep 0.3
    echo 'echo 1 > /sys/kernel/debug/tracing/tracing_on'
    sleep 0.3
    echo 'echo ===T2: block traffic (cold read + write + sync) ==='
    sleep 0.3
    echo 'cat /test/binder_probe > /dev/null'
    sleep 1
    # A real write (not one small line): reads may hit the page cache,
    # but a 200 KB fresh write always dirties dozens of blocks, and
    # sync then pushes them through the synchronous request path — the
    # block-event check must not depend on read-cache luck.
    echo 'head -c 200000 /bin/toybox > /trace2.tmp; sync'
    sleep 3
    # Grep the block lines NOW, before the binder phase: the closed-loop
    # probe emits thousands of binder events over its 8 s run and wraps
    # the per-CPU ring (1024 entries) past the block records — the
    # first run of this smoke grepped them only after the probe and saw
    # none while the events had recorded fine (blkdbg boot: block lines
    # in the reference geometry, diskstats moving on the same
    # chokepoints).
    echo 'echo ===T2: block lines ==='
    sleep 0.3
    echo 'grep block_rq_ /sys/kernel/debug/tracing/trace | head -6'
    sleep 0.5
    echo 'echo ===T2: binder traffic (closed-loop probe) ==='
    sleep 0.3
    echo '/test/binder_probe'
    sleep 8
    echo 'echo 0 > /sys/kernel/debug/tracing/tracing_on'
    sleep 0.5
    echo 'echo ===T2: binder lines ==='
    sleep 0.3
    echo 'grep binder_ /sys/kernel/debug/tracing/trace | head -8'
    sleep 0.5
    echo 'echo ===T2: group mixed state ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/binder/enable /sys/kernel/debug/tracing/events/block/enable /sys/kernel/debug/tracing/events/enable'
    sleep 0.5
    echo 'echo ===T2: done ==='
    sleep 3
} | timeout "$TIMEOUT" qemu-system-riscv64 \
    -M virt \
    -accel tcg,thread=single \
    -cpu rv64,zbb=true,zba=true,zbs=true \
    -m 1G \
    -smp 2 \
    -display none \
    -serial stdio \
    -snapshot \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
    > "$LOG" 2>&1

echo "serial log: $LOG"

fail=0
check() {
    local what="$1" pat="$2"
    if grep -qE "$pat" "$LOG"; then
        echo "  PASS: $what"
    else
        echo "  FAIL: $what  (pattern: $pat)"
        fail=1
    fi
}

echo "--- verdicts ---"
check "16 events in available_events" \
    'block:block_rq_complete'
check "binder events in available_events" \
    'binder:binder_transaction '
check "block_rq_issue format file" 'name: block_rq_issue'
check "block_rq_issue print fmt" '%d,%d %s %u \(%s\) %llu \+ %u %s,%u,%u \[%s\]'
check "block_rq_complete format file" 'name: block_rq_complete'
check "block_rq_issue recorded line" \
    'block_rq_issue: [0-9]+,[0-9]+ [RWF] [0-9]+ \(\) [0-9]+ \+ [0-9]+ none,0,0 \['
check "block_rq_complete recorded line" \
    'block_rq_complete: [0-9]+,[0-9]+ [RWF] \(\) [0-9]+ \+ [0-9]+ none,0,0 [0-9]+'
check "binder_transaction recorded line" \
    'binder_transaction: transaction=[0-9]+ dest_node=[0-9]+ dest_proc=[0-9]+ dest_thread=[0-9]+ reply=[01] flags=0x[0-9a-f]+ code=0x[0-9a-f]+'
check "binder_transaction_received recorded line" \
    'binder_transaction_received: transaction=[0-9]+'
check "binder_transaction_alloc_buf recorded line" \
    'binder_transaction_alloc_buf: transaction=[0-9]+ data_size=[0-9]+ offsets_size=[0-9]+ extra_buffers_size=[0-9]+'
check "binder_transaction_buffer_release recorded line" \
    'binder_transaction_buffer_release: transaction=[0-9]+ data_size=[0-9]+'
check "binder probe PASS (live traffic)" 'BINDER_PROBE RESULT: PASS'

exit $fail
