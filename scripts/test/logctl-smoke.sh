#!/bin/bash
# logctl-smoke.sh — log-control lane acceptance on x86_64 (q35, KVM):
# the runtime console-verbosity knob, the /proc/kmsg reference shape,
# and the inline panic banner.
#
# One QEMU, two boots (a guest reset does not clear RAM):
#   boot 1: init=/test/lx exercises the knobs from userspace, then
#           crashes via sysrq 'c' (panic=3 reboots the guest);
#   boot 2: the pstore console record resurfaces and the panic banner
#           in it must be `Kernel panic - not syncing: <message>` with
#           the message INLINE (no standalone `PANIC:` line).
#
# Checks, in order (guest side, asserted on the serial log):
#   1. /proc/sys/kernel/printk reads the four live numbers,
#      tab-separated like proc_dointvec ("6\t4\t1\t6")
#   2. a single-number write lowers the live console_loglevel (3)
#   3. at level 3 a KERN_INFO /dev/kmsg marker reaches the ring
#      (read back) but NOT the serial console; after restoring 6 the
#      same write emits live
#   4. syslog(2) action 8 moves the same value the file reads
#   5. /proc/kmsg: `<N>[SSSSS.MMMMMM] ` shape, no pid/cpu decoration,
#      fill reads (>= 2 records per read), single-reader EBUSY,
#      destructive consumption (window 2 never re-serves window 1)
#   6. the panic banner is one line with the message inline; the old
#      `PANIC:`-on-its-own-line shape is gone; panic=3 reboots
#
# The image is a fresh minimal ext4 (the pstore/pbench recipe: mkfs -d
# over a staging dir holding the static probe; the host gcc targets
# the amd64 guest directly). The probe source is restored from the
# userspace/probes archive (user rule 2026-11-02).
#
# Usage: scripts/test/logctl-smoke.sh [bzImage] [logfile]
# Env:   TIMEOUT (default 240)
#
# -m 2G, not 1G: the x86 layout's kernel heap starts at the 1 GiB mark
# (see scripts/test/pstore-x86-smoke.sh for the ALLOCTHROW-at-1G failure mode).

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BZIMAGE="${1:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
LOG="${2:-$ROOT/run/logctl-smoke-serial.log}"
TIMEOUT="${TIMEOUT:-240}"

PROBE_SRC="$ROOT/test/logctl_x86probe.c"
PROBE="$ROOT/work/logctl-x86-lx"
STAGING="$ROOT/work/logctl-x86-root"
IMG="$ROOT/work/logctl-x86.img"
MKFS=/usr/sbin/mkfs.ext4

fail=0
result() { echo "LOGCTL-RESULT: $1 $2"; case "$1" in
    PASS) ;;
    SKIP) exit 3 ;;
    *)    exit 1 ;;
esac }

[ -f "$BZIMAGE" ] && [ -x "$BZIMAGE" -o -f "$BZIMAGE" ] || result SKIP "x86 bzImage not built at $BZIMAGE (PLATFORM=x86_64 make build)"
command -v gcc >/dev/null 2>&1 || result SKIP "host gcc not found"
[ -x "$MKFS" ] || MKFS=$(command -v mkfs.ext4) || result SKIP "mkfs.ext4 not found"
[ -w /dev/kvm ] || result SKIP "/dev/kvm not available (KVM is the standard accel for this smoke)"

# Restore the probe source from the archive (idempotent, the rule's
# extraction path — the same thing userspace/probes/extract.sh does).
mkdir -p "$ROOT/run" "$ROOT/work"
tar xzf "$ROOT/userspace/probes/probes.tar.gz" -C "$ROOT" test/logctl_x86probe.c \
    || result FAIL "probe source missing from userspace/probes/probes.tar.gz"

gcc -static -O2 -Wall -o "$PROBE" "$PROBE_SRC" || result FAIL "probe build failed"

rm -rf "$STAGING"
mkdir -p "$STAGING/test"
cp "$PROBE" "$STAGING/test/lx"
chmod 755 "$STAGING/test/lx"
rm -f "$IMG"
"$MKFS" -q -F -O '^metadata_csum' -d "$STAGING" "$IMG" 96M || result FAIL "mkfs failed"

rm -f "$LOG"
{
    # the probe drives itself; the sleep only paces the log capture
    sleep "$TIMEOUT"
} | timeout "$TIMEOUT" qemu-system-x86_64 \
    -M q35 \
    -accel kvm \
    -cpu max \
    -m 2G \
    -smp 8 \
    -display none \
    -serial stdio \
    -snapshot \
    -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,drive=rootfs \
    -kernel "$BZIMAGE" \
    -append "root=/dev/vda rw init=/test/lx console=ttyS0 panic=3" \
    > "$LOG" 2>&1

echo "serial log: $LOG"

check() {
    local what="$1" pat="$2"
    if grep -a -qE "$pat" "$LOG"; then
        echo "  PASS: $what"
    else
        echo "  FAIL: $what  (pattern: $pat)"
        fail=1
    fi
}
nocheck() {
    local what="$1" pat="$2"
    if grep -a -qE "$pat" "$LOG"; then
        echo "  FAIL: $what  (forbidden pattern present: $pat)"
        fail=1
    else
        echo "  PASS: $what"
    fi
}

echo "--- verdicts ---"
# boot 1: the four-number knob
check "probe ran" '===LX1: logctl probe start==='
check "printk reads four tab-separated live numbers" $'LX: printk0=6\t4\t1\t6'
check "single-number write lowers console_loglevel" $'LX: printk_after3=3\t4\t1\t6'
# console gating: store yes, emit no. The scan is limited to BOOT 1's
# live region (probe start .. sysrq crash): boot 2 legitimately prints
# the pstore record, whose printk tail carries the ring-STORED marker —
# that is the store-not-emit feature, not a console leak.
if sed -n '/===LX1: logctl probe start===/,/===LX1: crashing/p' "$LOG" \
        | grep -a -q 'LXQUIET: below-console marker'; then
    echo "  FAIL: below-console marker emitted to the boot-1 serial (level 3)"
    fail=1
else
    echo "  PASS: below-console marker stayed off the boot-1 serial (level 3)"
fi
check "below-console marker reached the ring" 'LX: ring_has_quiet=1'
check "console restored to 6" $'LX: printk_restored=6\t4\t1\t6'
check "above-console marker emitted live (level 6)" 'LXLOUD: above-console marker'
# syslog(2) consistency
check "syslog(2) level action moved the file value" 'LX: syslog_consistent=1'
# /proc/kmsg shape and semantics
check "kmsg fill read returned multiple records" 'LX: km1_records=[2-9][0-9]*'
check "kmsg records carry the reference syslog shape" 'LX: km1_shape_ok=1'
check "kmsg records carry no pid/cpu decoration" 'LX: km_pid_deco=0'
check "kmsg second concurrent open is EBUSY" 'LX: km_second_open_ebusy=1'
check "kmsg window 2 serves the new marker" 'LX: km2_has_lxd=1'
check "kmsg consumption is destructive (no re-serve)" 'LX: km2_has_km1head=0'
# panic banner
check "panic banner inline: message on the banner line" '^Kernel panic - not syncing: .+'
nocheck "old standalone PANIC: line is gone" '^PANIC: '
# (no $ anchor: the serial line carries a trailing CR from the \r\n
# newline expansion)
check "end marker after the dump" '^---\[ end Kernel panic - not syncing \]---'
check "panic=3 reboot policy announced" 'panic: rebooting in 3s'
# boot 2
check "panic=3 rebooted (boot 2 ran)" '===LX2: pstore record resurfaced==='
check "pstore announce line" "pstore: previous boot.s console record found"
check "pstore record carries the inline banner" 'LX2: banner_inline=[1-9]'
check "pstore record has no old PANIC: line" 'LX2: old_panic_line=0'
check "pstore record carries the end marker" 'LX2: end_marker=[1-9]'

if [ "$fail" -eq 0 ]; then
    result PASS "all logctl checks green (see verdicts above)"
fi
exit $fail
