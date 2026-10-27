#!/bin/bash
# pstore-smoke.sh — pstore multi-record acceptance: the last crashes'
# records survive the reboots, read back through /sys/fs/pstore in
# newest-first order, and a fully-read record is consumed (read-once
# removal, the reference pstore's release contract).
#
# One QEMU, three boots (a guest reset does not clear RAM):
#   boot 1: panic=3 on the command line; the driver crashes the kernel
#           via sysrq 'c' (echo c > /proc/sysrq-trigger) — the record
#           (seq 1) is written to a slot and the guest reboots ~3 s
#           later;
#   boot 2: DOES NOT READ the record (an unread record must survive);
#           prints the PSBOOT2 marker (it lands in boot 2's printk ring
#           and therefore in boot 2's own record tail later), then
#           crashes again (seq 2);
#   boot 3: /sys/fs/pstore holds TWO files: console-ramoops-0 (newest,
#           boot 2's crash — its tail carries the PSBOOT2 marker) and
#           console-ramoops-1 (boot 1's crash, no marker). Both mode
#           0444. Reading console-ramoops-0 (one cp) consumes it: the
#           directory then lists only console-ramoops-1; reading that
#           consumes it too and the directory is empty.
#
# The probe image is a private copy of test/rootfs.img (never the
# shared one).
#
# Usage: test/pstore-smoke.sh [kernel] [logfile]
# Env:   BOOT_DELAY (default 30), TIMEOUT (default 360)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/riscv64gc-unknown-none-elf/debug/rux}"
LOG="${2:-$ROOT/run/pstore-smoke-serial.log}"
BOOT_DELAY="${BOOT_DELAY:-30}"
TIMEOUT="${TIMEOUT:-360}"

IMG="$ROOT/work/pstore-smoke.img"

mkdir -p "$ROOT/run" "$ROOT/work"
rm -f "$LOG" "$IMG"
cp "$ROOT/test/rootfs.img" "$IMG"

{
    # ---- boot 1: crash (seq 1) ---------------------------------------
    sleep "$BOOT_DELAY"
    echo 'echo ===PS: boot1 crashing via sysrq c ==='
    sleep 0.3
    echo 'echo c > /proc/sysrq-trigger'
    sleep 8
    # ---- boot 2: do NOT read; marker; crash again (seq 2) -------------
    echo 'echo ===PS: boot2 marking then crashing again ==='
    sleep 30
    echo 'echo 1 > /sys/kernel/debug/dfx/monowatch'
    sleep 0.5
    echo 'echo c > /proc/sysrq-trigger'
    sleep 8
    # ---- boot 3: two records, newest first, read-once -----------------
    echo 'echo ===PS: boot3 pstore dir - expect two records ==='
    sleep 30
    echo 'ls -l /sys/fs/pstore/'
    sleep 0.5
    echo 'echo ===PS: copy newest boot2 seq2 ==='
    sleep 0.3
    echo 'cp /sys/fs/pstore/console-ramoops-0 /tmp/r0'
    sleep 0.5
    echo 'echo ===PS: dir after first read read-once ==='
    sleep 0.3
    echo 'ls /sys/fs/pstore/'
    sleep 0.5
    echo 'echo ===PS: newest record checks ==='
    sleep 0.3
    echo 'echo PSDBG:newest_banner=$(grep -c "Kernel panic - not syncing" /tmp/r0)'
    sleep 0.3
    echo 'echo PSDBG:newest_trace=$(grep -c "Call Trace" /tmp/r0)'
    sleep 0.3
    echo 'echo PSDBG:newest_is_boot2=$(grep -c "runtime switch .monowatch. enabled" /tmp/r0)'
    sleep 0.3
    echo 'echo ===PS: copy oldest (boot1) ==='
    sleep 0.3
    echo 'cp /sys/fs/pstore/console-ramoops-1 /tmp/r1'
    sleep 0.5
    echo 'echo PSDBG:oldest_is_boot1=$(grep -c "runtime switch .monowatch. enabled" /tmp/r1)'
    sleep 0.3
    echo 'echo PSDBG:oldest_banner=$(grep -c "Kernel panic - not syncing" /tmp/r1)'
    sleep 0.3
    echo 'echo ===PS: dir empty after both reads ==='
    sleep 0.3
    echo 'ls /sys/fs/pstore/'
    sleep 0.5
    echo 'echo ===PS: done ==='
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
    -append "root=/dev/vda rw init=/bin/sh console=ttyS0 panic=3" \
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
check "boot1 panics (sysrq c)" 'sysrq: crashing the kernel on request'
banners=$(grep -c 'POSIX Compatible' "$LOG")
if [ "${banners:-0}" -ge 3 ]; then
    echo "  PASS: panic=3 reboots (boot banner x$banners)"
else
    echo "  FAIL: panic=3 reboots (banner count $banners < 3)"
    fail=1
fi
found=$(grep -c 'pstore: previous boot.s console record found' "$LOG")
if [ "${found:-0}" -ge 3 ]; then
    echo "  PASS: found-record lines (boot2 x1 + boot3 x2 = $found)"
else
    echo "  FAIL: found-record lines (count $found < 3)"
    fail=1
fi
check "two records listed, both mode 0444" '^-r--r--r--.*console-ramoops-[01]'

# Region helper: the serial lines between two markers.
region() { awk -v a="$1" -v b="$2" 'index($0,a){f=1;next} index($0,b){f=0} f' "$LOG"; }

if region 'dir after first read' 'newest record checks' | grep -q '^console-ramoops-1'; then
    echo "  PASS: read-once removed the newest, oldest still listed"
else
    echo "  FAIL: read-once removal (expected only console-ramoops-1 after first read)"
    fail=1
fi
check "newest record carries the printed banner" 'PSDBG:newest_banner=[1-9]'
check "newest record carries the symbolized stack header" 'PSDBG:newest_trace=[1-9]'
check "newest record is boot2's - monowatch knob line in tail" 'PSDBG:newest_is_boot2=[1-9]'
check "oldest record is boot1's - no monowatch line" 'PSDBG:oldest_is_boot1=0'
check "oldest record carries the printed banner" 'PSDBG:oldest_banner=[1-9]'
if region 'dir empty after both reads' 'PS: done' | grep -q 'console-ramoops'; then
    echo "  FAIL: directory not empty after both reads"
    fail=1
else
    echo "  PASS: directory empty after both reads"
fi

exit $fail
