#!/bin/bash
# tr2-kvm-proof.sh — dfx-trace round 2 serial proof on x86_64+KVM.
#
#   Boot A (no dfx=): the full round-2 flow from run/tr2-proof-init.c —
#     P0 boot state (empty set_event, tracing_on 0)
#     P1 snapshot file present
#     P2 kmem family formats + group arming
#     P3a kmem record lines (kmalloc/kfree/mm_page_alloc/mm_page_free,
#         symbolized origins)
#     P3b sched_waking/sched_wakeup recorded by the round-2 sites
#     P3c snapshot save/clear round-trip
#     P4 per-family enabled differentials (kmem, sched, raw_syscalls)
#   Boot B (dfx=schedtrace): the switch-alias proof — the legacy
#     `dfx=` switch pre-arms the sched:* events and tracing_on at boot
#     (hunting scripts keep working, now with the event timeline).
#
# Usage: run/tr2-kvm-proof.sh [bzImage] [logfile-prefix]
# Env:   TIMEOUT_A (default 300), TIMEOUT_B (default 90)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${1:-$ROOT/target/x86_64-unknown-none/release/bzImage}"
PREFIX="${2:-$ROOT/run/tr2-kvm-proof}"
TIMEOUT_A="${TIMEOUT_A:-300}"
TIMEOUT_B="${TIMEOUT_B:-90}"
ROOTFS="$ROOT/run/tr2-rootfs.img"

mkdir -p "$ROOT/run"

boot() {
    local log="$1"; shift
    local timeout_s="$1"; shift
    local extra_append="$1"; shift
    rm -f "$log"
    {
        sleep "$timeout_s"
    } | timeout "$timeout_s" qemu-system-x86_64 \
        -M q35 \
        -accel kvm \
        -cpu max \
        -m 2G \
        -smp 2 \
        -display none \
        -serial stdio \
        -snapshot \
        -drive file="$ROOTFS",if=none,id=rootfs,format=raw,file.locking=off \
        -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
        -kernel "$KERNEL" \
        -append "root=/dev/vda rw init=/init console=ttyS0 $extra_append" \
        > "$log" 2>&1
    echo "serial log: $log"
}

boot "$PREFIX-serial.log" "$TIMEOUT_A" ""
grep -n "===P\|KMEMDBG\|WAKEDBG\|SNAPDBG\|PROOF-INIT-DONE\|KERNPANIC\|panic" \
    "$PREFIX-serial.log" | head -30

boot "$PREFIX-alias-serial.log" "$TIMEOUT_B" "dfx=schedtrace"
echo "--- alias boot (dfx=schedtrace): P0 shows the sched family pre-armed ---"
sed -n '/\[P0\] boot state/,/^\[P1\]/p' "$PREFIX-alias-serial.log" | head -30
grep -n "dfx: runtime switch\|PROOF-INIT-DONE\|KERNPANIC\|panic" \
    "$PREFIX-alias-serial.log" | head -10
