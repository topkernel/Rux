#!/bin/bash
# blktrace-smoke.sh — the debug-blktrace lane's acceptance run on
# x86_64 (q35, KVM): the three storage rows of dfx-align-plan §3.
#
# Boots the kernel on a fresh minimal ext4 image with the lane's probe
# compiled in (host gcc -static; the amd64 guest is the host arch — the
# logctl-smoke recipe) and asserts on the probe's PASS/FAIL lines:
#
#   1. /proc/slabinfo — reference 5.15 geometry (version 2.1 header,
#      per-page object counts per class, SLUB zero tunables, slabdata
#      match), mode 0400, and LIVE counters (file churn moves a row)
#   2. /proc/pagetypeinfo — absent (blocked-on-design: no migratetype
#      free lists; the probe asserts ENOENT, pinning the row)
#   3. /sys/kernel/debug/block/<dev>/ — enable reads 0 at boot, 1/0
#      toggles, garbage write is EINVAL; trace readback is EMPTY
#      before arming, carries dev-matched block_rq records under arm
#      (in the reference tracefs line geometry), and goes SILENT after
#      disarm (count unchanged across a post-disarm IO burst); the
#      readback and the global trace agree on content
#
# Host shape reference (kernel 7.0.0-42, captured with the two block
# events armed via tracefs):
#     sync-2385881 [030] ..... 139146.910168: block_rq_issue: 8,16 RM 4096 () 1050853656 + 8 be,0,4 [sync]
#   <idle>-0 [025] ..s1. 139146.910492: block_rq_complete: 8,16 RM () 1050853656 + 8 be,0,4 [0]
# The guest's first readback line is printed as BLK_PROBE trace-first:
# and must match that shape (the same renderer the tracefs trace file
# uses). The rwbs divergence (Rux carries direction only: R/W/F, the
# reference's RM/WS sync+meta bits have no counterpart) is documented
# in the event definition.
#
# Usage: scripts/test/blktrace-smoke.sh [bzImage] [logfile]
# Env:   TIMEOUT (default 180)
#
# -m 2G, not 1G: the x86 layout's kernel heap starts at the 1 GiB mark
# (the pstore-x86-smoke ALLOCTHROW-at-1G failure mode).

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BZIMAGE="${1:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
LOG="${2:-$ROOT/run/blktrace-smoke-serial.log}"
TIMEOUT="${TIMEOUT:-180}"

PROBE_SRC="$ROOT/test/blktrace_x86probe.c"
PROBE="$ROOT/work/blktrace-x86-bx"
STAGING="$ROOT/work/blktrace-x86-root"
IMG="$ROOT/work/blktrace-x86.img"
MKFS=/usr/sbin/mkfs.ext4

fail=0
result() { echo "BLKTRACE-RESULT: $1 $2"; case "$1" in
    PASS) ;;
    SKIP) exit 3 ;;
    *)    exit 1 ;;
esac }

[ -f "$BZIMAGE" ] || result SKIP "x86 bzImage not built at $BZIMAGE (PLATFORM=x86_64 make build)"
command -v gcc >/dev/null 2>&1 || result SKIP "host gcc not found"
[ -x "$MKFS" ] || MKFS=$(command -v mkfs.ext4) || result SKIP "mkfs.ext4 not found"
[ -w /dev/kvm ] || result SKIP "/dev/kvm not available (KVM is the standard accel for this smoke)"

# Restore the probe source from the archive (user rule 2026-11-02 —
# the same extraction the logctl smoke does).
mkdir -p "$ROOT/run" "$ROOT/work"
tar xzf "$ROOT/userspace/probes/probes.tar.gz" -C "$ROOT" test/blktrace_x86probe.c \
    || result FAIL "probe source missing from userspace/probes/probes.tar.gz"

gcc -static -O2 -Wall -o "$PROBE" "$PROBE_SRC" || result FAIL "probe build failed"

rm -rf "$STAGING"
mkdir -p "$STAGING/test"
cp "$PROBE" "$STAGING/test/bx"
chmod 755 "$STAGING/test/bx"
rm -f "$IMG"
"$MKFS" -q -F -O '^metadata_csum' -d "$STAGING" "$IMG" 96M || result FAIL "mkfs failed"

rm -f "$LOG"
{
    # The probe is init and drives itself; the sleep paces the capture.
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
    -append "root=/dev/vda rw init=/test/bx console=ttyS0" \
    > "$LOG" 2>&1

echo "serial log: $LOG"

check() {
    local what="$1" pat="$2"
    if LC_ALL=C grep -aEq "$pat" "$LOG"; then
        echo "  PASS: $what"
    else
        echo "  FAIL: $what  (pattern: $pat)"
        fail=1
    fi
}

echo "--- verdicts ---"
# The probe prints one BLK_PROBE <what>: PASS/FAIL line per check —
# assert every one of them, plus the probe's own RESULT and end marker.
for v in slabinfo-mode-0400 slabinfo-header slabinfo-rowcount \
         slabinfo-geometry slabinfo-live-counters slabinfo-reparse \
         pagetypeinfo-absent block-dir \
         enable-read-0 trace-empty-before-arm enable-write-1 \
         enable-read-1 event-switch-armed trace-records-under-arm \
         diskstats-match trace-geometry-devmatch \
         trace-vs-global enable-write-0 enable-read-0-again \
         event-switch-disarmed trace-silent-after-disarm \
         enable-einval enable-state-after-einval; do
    check "probe $v" "BLK_PROBE $v: PASS"
done
check "probe overall PASS"   'BLK_PROBE RESULT: PASS'
check "probe reached the end" '===BLK-SMOKE-DONE==='
# The shape record: the first readback line must carry the reference
# tracefs geometry (TASK-PID [cpu] flags sec.usec: event: args) and a
# bracketed complete error — the 7.0.0 host's tail shape.
check "readback line geometry" 'BLK_PROBE trace-first: +.+ \[[0-9]+\] +[.a-z0-9]{5} +[0-9]+\.[0-9]{6}: block_rq_(issue|complete): +[0-9]+,[0-9]+ '
if LC_ALL=C grep -aqE "Kernel panic|Oops:|BUG:" "$LOG"; then
    echo "  FAIL: serial not clean (panic/Oops/BUG present)"
    fail=1
else
    echo "  PASS: serial clean (no panic/Oops/BUG)"
fi

exit $fail
