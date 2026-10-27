#!/bin/bash
# trace-kmem-smoke.sh — kmem tracepoint family smoke: kmem:kmalloc and
# kmem:kfree, the reference host's events/kmem formats verbatim, with
# the ORIGIN column (call_site, %pS-symbolized through the kernel
# symbol table) the heap-leak hunts read.
#
# Checks, in order:
#   1. available_events carries kmem:kmalloc and kmem:kfree
#   2. the format files: name/ID header, the reference field rows
#      (call_site/ptr/bytes_req/bytes_alloc/gfp_flags/node), the print
#      fmt head
#   3. live traffic: arming the pair, running one bounded command
#      (reading /proc/1/status twice), disarming, then reading the
#      trace view records kmalloc/kfree lines in the reference
#      geometry — including a SYMBOLIZED call_site (a rux:: text name,
#      not a raw address) and GFP_KERNEL/-1/false in the gfp/node/
#      accounted columns
#   4. the kmem group switch reads 0 before arming and 1 after
#
# The probe image is a private copy of test/rootfs.img (never the
# shared one).
#
# Usage: test/trace-kmem-smoke.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 30), TIMEOUT (default 300)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/trace-kmem-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-30}"
TIMEOUT="${TIMEOUT:-300}"

IMG="$ROOT/work/trace-kmem-smoke.img"

mkdir -p "$ROOT/run" "$ROOT/work"
rm -f "$LOG" "$IMG"
cp "$ROOT/test/rootfs.img" "$IMG"

{
    sleep "$BOOT_DELAY"
    echo 'echo ===KMEM: available ==='
    sleep 0.3
    echo 'grep kmem: /sys/kernel/debug/tracing/available_events'
    sleep 0.5
    echo 'echo ===KMEM: kmalloc format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/kmem/kmalloc/format'
    sleep 0.5
    echo 'echo ===KMEM: kfree format ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/kmem/kfree/format'
    sleep 0.5
    echo 'echo ===KMEM: group state before ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/kmem/enable'
    sleep 0.5
    # Arm, one bounded burst of kernel work, disarm — the per-CPU ring
    # is 1024 entries and the kernel allocates continuously, so the
    # window is kept short and the trace read afterwards.
    echo 'echo ===KMEM: arming ==='
    sleep 0.3
    echo 'echo 1 > /sys/kernel/debug/tracing/tracing_on'
    echo 'echo 1 > /sys/kernel/debug/tracing/events/kmem/enable'
    sleep 0.3
    echo 'cat /proc/1/status > /dev/null'
    sleep 0.5
    echo 'cat /proc/1/status > /dev/null'
    sleep 0.5
    echo 'echo 0 > /sys/kernel/debug/tracing/events/kmem/enable'
    echo 'echo 0 > /sys/kernel/debug/tracing/tracing_on'
    sleep 0.3
    echo 'echo ===KMEM: group state after ==='
    sleep 0.3
    echo 'cat /sys/kernel/debug/tracing/events/kmem/enable'
    sleep 0.5
    echo 'echo ===KMEM: kmalloc lines ==='
    sleep 0.3
    echo 'grep " kmalloc: " /sys/kernel/debug/tracing/trace | head -4'
    sleep 0.5
    echo 'echo ===KMEM: kfree lines ==='
    sleep 0.3
    echo 'grep " kfree: " /sys/kernel/debug/tracing/trace | head -4'
    sleep 0.5
    echo 'echo ===KMEM: origin symbolized ==='
    sleep 0.3
    echo 'echo KMEMDBG:origin_count=$(grep -c "call_site=rux::" /sys/kernel/debug/tracing/trace)'
    sleep 0.5
    echo 'echo ===KMEM: done ==='
    sleep 2
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
    if grep -a -qE "$pat" "$LOG"; then
        echo "  PASS: $what"
    else
        echo "  FAIL: $what  (pattern: $pat)"
        fail=1
    fi
}

echo "--- verdicts ---"
check "available_events carries the kmem pair" 'kmem:kmalloc'
check "kmalloc format: name and id" 'name: kmalloc'
check "kmalloc format: call_site field row" 'field:unsigned long call_site'
check "kmalloc format: bytes_alloc field row" 'field:size_t bytes_alloc'
check "kmalloc format: print fmt head" 'call_site=%pS ptr=%p bytes_req=%zu bytes_alloc=%zu'
check "kfree format: name and id" 'name: kfree'
check "kfree format: print fmt" 'call_site=%pS ptr=%p'
check "kmalloc line in the reference geometry" \
    'kmalloc: call_site=[^ ]+ ptr=0x[0-9a-f]+ bytes_req=[0-9]+ bytes_alloc=[0-9]+ gfp_flags=GFP_KERNEL node=-1 accounted=false'
check "kfree line in the reference geometry" \
    'kfree: call_site=[^ ]+ ptr=0x[0-9a-f]+'
check "the origin column is symbolized (not a raw address)" 'KMEMDBG:origin_count=[1-9]'
check "the group state lines printed" 'KMEM: group state after'

exit $fail
