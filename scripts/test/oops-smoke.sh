#!/bin/bash
# oops-smoke.sh — Tranche 5 remaining bullets + the T7 perf-support
# slice, guest acceptance:
#
#   1. the non-fatal Oops path — a provoked kernel page fault from a
#      task context prints the Linux-shaped report (head line, Oops
#      banner, Tainted: G D, the faulting frame, Call Trace), the
#      machine SURVIVES (the next command still runs), and the record
#      lands in pstore as the dmesg flavor — after the reboot
#      /sys/fs/pstore/dmesg-ramoops-0 carries the report;
#   2. the persistent log — klogd starts, /var/log/kmsg archives the
#      boot log (non-empty, recent seq numbers);
#   3. per-task schedstat — the sysctl flips 0→1, and a live task's
#      /proc/[pid]/schedstat renders the three-number triple with a
#      nonzero timeslice count.
#
# One QEMU, two boots (a guest reset does not clear RAM):
#   boot 1: checks 2+3, then the oops, then sysrq 'b' to reset;
#   boot 2: the pstore found-record line names the dmesg flavor and
#           the file reads back the oops banner.
#
# The probe image is a private copy of test/rootfs.img.
#
# Usage: test/oops-smoke.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 30), TIMEOUT (default 420)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/oops-smoke-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-30}"
TIMEOUT="${TIMEOUT:-480}"

IMG="$ROOT/work/oops-smoke.img"

mkdir -p "$ROOT/run" "$ROOT/work"
rm -f "$LOG" "$IMG"
cp "$ROOT/test/rootfs.img" "$IMG"

{
    # ---- boot 1 ------------------------------------------------------
    sleep "$BOOT_DELAY"
    echo 'echo ===OS: boot1 ==='
    sleep 0.5
    # (2) persistent log
    echo 'echo ===OS: klogd + /var/log/kmsg ==='
    sleep 0.3
    echo 'ls -l /var/log/kmsg'
    sleep 0.5
    echo 'echo OSDBG:klog_lines=$(wc -l < /var/log/kmsg)'
    sleep 0.5
    echo 'echo OSDBG:klog_tail=$(tail -1 /var/log/kmsg)'
    sleep 0.5
    # The periodic flush: klogd's init flush landed at ~1.4 s; its first
    # 30 s wake lands at ~31.4 s GUEST time. Host-side sleeps measure
    # wall time, and under a loaded host the TCG guest runs slower than
    # realtime — the first run's sleep 12 missed the wake (r4: file
    # still 3 lines at host t~44 s while a concurrent smoke showed the
    # flush landing later; klogd-probe2 confirmed the flush itself is
    # clean). Two periods of host-side margin make the check
    # load-robust.
    sleep 60
    echo 'echo OSDBG:klog_lines2=$(wc -l < /var/log/kmsg)'
    sleep 0.5
    # (3) per-task schedstat
    echo 'echo ===OS: schedstat ==='
    sleep 0.3
    echo 'echo OSDBG:gate0=$(cat /proc/sys/kernel/sched_schedstats)'
    sleep 0.3
    echo 'echo 1 > /proc/sys/kernel/sched_schedstats'
    sleep 0.3
    echo 'echo OSDBG:gate=$(cat /proc/sys/kernel/sched_schedstats)'
    sleep 0.3
    echo 'sh -c "sleep 4" &'
    sleep 1
    echo 'read a b c < /proc/$!/schedstat 2>/dev/null || read a b c < /proc/self/schedstat; echo OSDBG:schedstat="[$a|$b|$c]"'
    sleep 3
    echo 'echo ===OS: schedstat raw self ==='
    sleep 0.3
    echo 'cat /proc/self/schedstat'
    sleep 0.5
    echo 'echo 0 > /proc/sys/kernel/sched_schedstats'
    sleep 0.3
    # (1) the oops
    echo 'echo ===OS: provoking kernel oops (child shell dies) ==='
    sleep 0.3
    echo 'sh -c "echo pagefault > /sys/kernel/debug/dfx/provoke-oops"'
    sleep 2
    echo 'echo ===OS: SURVIVED-the-oops marker ==='
    sleep 0.5
    echo 'echo OSDBG:tainted=$(cat /proc/sys/kernel/tainted)'
    sleep 0.5
    echo 'echo OSDBG:panic_on_oops=$(cat /proc/sys/kernel/panic_on_oops)'
    sleep 0.5
    echo 'echo ===OS: resetting for boot 2 ==='
    sleep 0.3
    echo 'echo b > /proc/sysrq-trigger'
    sleep 8
    # ---- boot 2: the dmesg record resurfaces --------------------------
    sleep 30
    echo 'echo ===OS: boot2 pstore ==='
    sleep 0.5
    echo 'ls -l /sys/fs/pstore/'
    sleep 0.5
    echo 'echo ===OS: record head ==='
    sleep 0.3
    echo 'head -8 /sys/fs/pstore/dmesg-ramoops-0'
    sleep 0.5
    echo 'echo OSDBG:oops_banner=$(grep -c "Oops: non-fatal kernel fault" /sys/fs/pstore/dmesg-ramoops-0)'
    sleep 0.5
    echo 'echo OSDBG:null_deref=$(grep -c "Unable to handle kernel NULL pointer dereference" /sys/fs/pstore/dmesg-ramoops-0)'
    sleep 0.5
    echo 'echo OSDBG:oops_count=$(grep -c "^Oops \[#" /sys/fs/pstore/dmesg-ramoops-0)'
    sleep 0.5
    echo 'echo OSDBG:tainted_line=$(grep -c "Tainted: G      D" /sys/fs/pstore/dmesg-ramoops-0)'
    sleep 0.5
    echo 'echo OSDBG:trace=$(grep -c "Call Trace" /sys/fs/pstore/dmesg-ramoops-0)'
    sleep 1
    echo 'echo ===OS: done ==='
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
    if grep -qE "$pat" "$LOG"; then
        echo "  PASS: $what"
    else
        echo "  FAIL: $what  (pattern: $pat)"
        fail=1
    fi
}

echo "--- verdicts ---"
# boot 1
check "klogd wrote /var/log/kmsg (non-empty)" 'OSDBG:klog_lines=[1-9]'
check "/var/log/kmsg line shape" 'OSDBG:klog_tail=[0-9]+ [0-9]+ [0-9]+ [0-9]+ [0-9] '
check "klogd's 30 s periodic flush grew the file" 'OSDBG:klog_lines2=[1-9][0-9]+'
check "schedstat gate reads 0 before arming" 'OSDBG:gate0=0'
check "schedstat gate arms" 'OSDBG:gate=1'
check "schedstat triple renders (nonzero timeslices)" 'OSDBG:schedstat=\[[0-9]+ [0-9]+ [1-9]'
check "schedstat raw self renders the triple" '^[0-9]+ [0-9]+ [0-9]+'
check "oops head line prints (NULL pointer dereference)" 'Unable to handle kernel NULL pointer dereference at virtual address'
check "oops banner prints with the counter" '^Oops \[#1\]'
check "oops taints D (Tainted line)" 'Tainted: G      D'
check "oops prints the faulting frame" 'epc ?: ?[0-9a-f]{16} ra : [0-9a-f]{16} sp : [0-9a-f]{16}'
check "oops prints the Call Trace" '^Call Trace:'
check "the machine survived the oops" 'SURVIVED-the-oops marker'
check "taint D lingers after the oops" 'OSDBG:tainted=128'
check "panic_on_oops sysctl reads 0" 'OSDBG:panic_on_oops=0'
# boot 2
banners=$(grep -c 'POSIX Compatible' "$LOG")
if [ "${banners:-0}" -ge 2 ]; then
    echo "  PASS: sysrq b reboots (boot banner x$banners)"
else
    echo "  FAIL: sysrq b reboots (banner count $banners < 2)"
    fail=1
fi
check "boot2 finds the record (the pstore boot line, dmesg flavor)" "pstore: previous boot.s dmesg record found"
check "record file exists, mode 0444" '^-r--r--r--.*dmesg-ramoops-0'
check "record carries the oops identity" 'OSDBG:oops_banner=[1-9]'
check "record carries the head line" 'OSDBG:null_deref=[1-9]'
check "record carries the banner" 'OSDBG:oops_count=[1-9]'
check "record carries the Tainted line" 'OSDBG:tainted_line=[1-9]'
check "record carries the Call Trace" 'OSDBG:trace=[1-9]'

exit $fail
