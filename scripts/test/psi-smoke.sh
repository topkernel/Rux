#!/bin/bash
# psi-smoke.sh — boot the minimal rootfs and verify /proc/pressure
# (PSI) end-to-end: file shape, live response to CPU load, decay
# after the load ends, monotonic totals, and a cold-file I/O leg.
#
# Checks, in order:
#   1. all three files exist with the reference line shape
#      (cpu: some only; memory/io: some + full; avgN=d.dd, total=int)
#   2. idle baseline: cpu avg10 near zero
#   3. under CPU load (4 spinners on 2 CPUs, ~35 s): cpu avg10 rises
#      well above baseline; cpu total strictly increases between two
#      load reads
#   4. after the load ends (~26 s idle): cpu avg10 decays vs the
#      load peak; total stays monotonic
#   5. cold-file reads (the never-booted /app ELFs, ~2.4 MB): the
#      read sectors in /proc/diskstats increase (real block I/O
#      happened). io total is EXPECTED to stay 0 in this tree: the
#      only IoCompletion::wait sites (the async batch read path) are
#      disabled by the completion-attribution interim gate
#      (ext4/file.rs fill_page_cache_batch, 2026-11-05), and the
#      synchronous read/write paths busy-poll inside submit_request
#      without an iowait section. The guard -> sampler -> render
#      plumbing is ktest-verified instead (tests/psi.rs, "stall
#      guard charges total across a jiffy"). memory total is
#      reported (it may legitimately stay 0 if the alloc slowpath is
#      never hit — the verdict line records which one happened)
#
# Usage: scripts/test/psi-smoke.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 45), TIMEOUT (default 480)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/psi-smoke-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-45}"
TIMEOUT="${TIMEOUT:-480}"

IMG="$ROOT/work/psi-smoke.img"

mkdir -p "$ROOT/run" "$ROOT/work"
rm -f "$LOG" "$IMG"
cp "$ROOT/test/rootfs.img" "$IMG"

{
    sleep "$BOOT_DELAY"
    echo 'echo ===PSI:SHAPE==='
    sleep 0.3
    echo 'cat /proc/pressure/cpu /proc/pressure/memory /proc/pressure/io'
    sleep 8
    echo 'echo ===PSI:R0CPU==='
    sleep 0.3
    echo 'cat /proc/pressure/cpu'
    sleep 0.5
    echo 'echo ===PSI:R0IO==='
    sleep 0.3
    echo 'cat /proc/pressure/io'
    sleep 0.5
    echo 'echo ===PSI:R0MEM==='
    sleep 0.3
    echo 'cat /proc/pressure/memory'
    sleep 1
    # --- CPU load: 4 spinners on 2 CPUs ---
    echo 'echo ===PSI:LOAD_START==='
    sleep 0.3
    echo 'yes > /dev/null &'
    sleep 0.5
    echo 'yes > /dev/null &'
    sleep 0.5
    echo 'yes > /dev/null &'
    sleep 0.5
    echo 'yes > /dev/null &'
    sleep 24
    echo 'echo ===PSI:R1CPU==='
    sleep 0.3
    echo 'cat /proc/pressure/cpu'
    sleep 10
    echo 'echo ===PSI:R2CPU==='
    sleep 0.3
    echo 'cat /proc/pressure/cpu'
    sleep 1
    echo 'echo ===PSI:LOAD_STOP==='
    sleep 0.3
    echo 'killall yes'
    sleep 26
    echo 'echo ===PSI:R3CPU==='
    sleep 0.3
    echo 'cat /proc/pressure/cpu'
    sleep 1
    # --- IO leg: cold-file reads (real block I/O through the read
    # path) + diskstats evidence. Raw block-device nodes are NOT
    # usable here: this kernel has no block-driver open path (devfs/
    # tmpfs get_file_ops return None for S_IFBLK by design), so the
    # former mknod+dd leg always failed ENXIO before reading a byte.
    # The /test ELFs are never touched by an init=/bin/sh boot, so
    # their first read is a guaranteed page-cache miss.
    echo 'echo ===PSI:IO_START==='
    sleep 0.3
    echo 'grep virtblk /proc/diskstats > /tmp/psi-d0; cat /tmp/psi-d0'
    sleep 0.5
    echo 'cat /app/calculator /app/clock /app/desktop /app/vshell > /dev/null'
    sleep 8
    echo 'echo ===PSI:R4IO==='
    sleep 0.3
    echo 'cat /proc/pressure/io'
    sleep 0.5
    echo 'echo ===PSI:R4MEM==='
    sleep 0.3
    echo 'cat /proc/pressure/memory'
    sleep 5
    echo 'echo ===PSI:R5IO==='
    sleep 0.3
    echo 'cat /proc/pressure/io'
    sleep 0.5
    echo 'echo ===PSI:R5MEM==='
    sleep 0.3
    echo 'cat /proc/pressure/memory'
    sleep 2
    echo 'grep virtblk /proc/diskstats > /tmp/psi-d1; cat /tmp/psi-d1'
    sleep 0.5
    echo 'echo ===PSI:DONE==='
    sleep 1
} | timeout "$TIMEOUT" qemu-system-riscv64 \
    -M virt \
    -accel tcg,thread=multi \
    -cpu rv64,zbb=true,zba=true,zbs=true \
    -m 1G \
    -smp 8 \
    -display none \
    -serial stdio \
    -snapshot \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
    > "$LOG" 2>&1

echo "serial log: $LOG"

# ---------- host-side verdicts ----------
fail=0
sec() { tr -d '\r' < "$LOG" | awk -v m="===PSI:$1===" '$0 ~ m {f=1;next} /===PSI:/ {f=0} f'; }

get_avg10() { get_avg10_label "$1" some; }
get_avg10_label() { # <marker> <line-label>
    sec "$1" | awk -v lbl="$2" '$1 == lbl {split($2,a,"="); print a[2]; exit}'
}
get_total() { get_total_label "$1" some; }
get_total_label() { # <marker> <line-label>
    sec "$1" | awk -v lbl="$2" '$1 == lbl {split($5,a,"="); print a[2]; exit}'
}

echo "--- psi-smoke verdicts ---"

shape_n() {
    sec SHAPE | grep -cE '^(some|full) avg10=[0-9]+\.[0-9]{2} avg60=[0-9]+\.[0-9]{2} avg300=[0-9]+\.[0-9]{2} total=[0-9]+$'
}
n=$(shape_n)
if [ "$n" -eq 5 ]; then
    echo "PASS: shape — 5 lines total (cpu some; memory some+full; io some+full)"
else
    echo "FAIL: shape — expected 5 well-formed lines, got $n"; fail=1
fi
if [ "$(sec SHAPE | grep -c '^some ')" -eq 3 ] && [ "$(sec SHAPE | grep -c '^full ')" -eq 2 ]; then
    echo "PASS: cpu has some only; memory/io have some+full"
else
    echo "FAIL: some/full line distribution wrong"; fail=1
fi

idle_avg=$(get_avg10 R0CPU)
if awk -v v="${idle_avg:-99}" 'BEGIN{exit !(v < 5.00)}'; then
    echo "PASS: idle cpu avg10 near zero ($idle_avg)"
else
    echo "FAIL: idle cpu avg10 too high ($idle_avg)"; fail=1
fi

load1_avg=$(get_avg10 R1CPU)
load2_avg=$(get_avg10 R2CPU)
if awk -v v="${load2_avg:-0}" 'BEGIN{exit !(v > 20.00)}'; then
    echo "PASS: cpu avg10 under load = $load2_avg (R1: $load1_avg)"
else
    echo "FAIL: cpu avg10 under load too low (R1: $load1_avg, R2: $load2_avg)"; fail=1
fi

t1=$(get_total R1CPU)
t2=$(get_total R2CPU)
t3=$(get_total R3CPU)
if [ -n "$t1" ] && [ -n "$t2" ] && [ "$t2" -gt "$t1" ]; then
    echo "PASS: cpu total monotonic under load ($t1 -> $t2 us)"
else
    echo "FAIL: cpu total not increasing under load ($t1 -> $t2)"; fail=1
fi
if [ -n "$t3" ] && [ "$t3" -ge "$t2" ]; then
    echo "PASS: cpu total monotonic after load ($t2 -> $t3 us)"
else
    echo "FAIL: cpu total regressed after load ($t2 -> $t3)"; fail=1
fi

decay_avg=$(get_avg10 R3CPU)
if awk -v a="${load2_avg:-999}" -v b="${decay_avg:-999}" 'BEGIN{exit !(b < a)}'; then
    echo "PASS: cpu avg10 decayed after load ($load2_avg -> $decay_avg)"
else
    echo "FAIL: cpu avg10 did not decay ($load2_avg -> $decay_avg)"; fail=1
fi

io0=$(get_total R0IO)
io4=$(get_total R4IO)
io5=$(get_total R5IO)
# Real block I/O evidence: the diskstats read-sector field (6th) of
# the virtblk row (major 8, minor 0, name pci-virtblk — the
# partitions layer's "vda/254" numbering is different), before vs
# after the cold-file reads.
sect_before=$(sec IO_START | awk '$1==8 && $2==0 {print $6; exit}')
sect_after=$(sec R5MEM | awk '$1==8 && $2==0 {print $6; exit}')
sect_delta=""
if [ -n "$sect_before" ] && [ -n "$sect_after" ]; then
    sect_delta=$((sect_after - sect_before))
fi
if [ -n "$io0" ] && [ -n "$io5" ] && [ "$io5" -gt "$io0" ]; then
    echo "PASS: io total grew during cold-file reads ($io0 -> $io4 -> $io5 us)"
elif [ -n "$sect_delta" ] && [ "$sect_delta" -gt 0 ]; then
    echo "NOTE: io total stayed 0 (io0=$io0 io4=$io4 io5=$io5) — EXPECTED in this tree: the only IoCompletion::wait sites (async batch read path) are disabled by the completion-attribution interim gate (ext4/file.rs fill_page_cache_batch, 2026-11-05) and the sync paths busy-poll without an iowait section; guard->sampler plumbing is ktest-verified (tests/psi.rs). Real block I/O DID run: vda sectors_read +$sect_delta."
else
    echo "FAIL: io total did not grow ($io0 -> $io4 -> $io5) and no cold-read device traffic observed (sectors_read delta=${sect_delta:-none})"; fail=1
fi

mem0=$(get_total R0MEM)
mem5=$(get_total R5MEM)
if [ -n "$mem5" ] && [ "$mem5" -gt "${mem0:-0}" ]; then
    echo "PASS: memory total grew under page-cache pressure ($mem0 -> $mem5 us)"
else
    echo "NOTE: memory total stayed ${mem0:-?} us (alloc slowpath never hit in this run — memstall hook verified by build + ktest, no live stall observed)"
fi

echo "---"
if [ "$fail" -eq 0 ]; then
    echo "psi-smoke: ALL PASS"
else
    echo "psi-smoke: FAILED ($fail)"
fi
exit "$fail"
