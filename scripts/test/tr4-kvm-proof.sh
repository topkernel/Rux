#!/bin/bash
# tr4-kvm-proof.sh — dfx-trace round 4 serial proof on x86_64+KVM.
#
#   Boot A (no debug=): the full round-4 flow from run/tr4-proof-init.c —
#     P0 boot state (empty set_event, tracing_on 0)
#     P1 catalog (AVAIL10), formats, lock/tcp/sock group arming,
#        per-event enable
#     P2 lock records under a real contention storm (4 children on one
#        directory — the SleepMutex big locks + the GRQ spinlock)
#     P3 net records: loopback lifecycle (state machine + destroy),
#        blackhole connect (retransmits), slirp-refused (receive_reset)
#     P4 enabled-vs-disabled differentials (lock churn, TCP lifecycle)
#     P5 trace_pipe drain
#   Boot B (debug=syncguard): the alias pre-arms exactly the lock: family.
#   Boot C (debug=tcptrace): the alias pre-arms exactly tcp:+sock:.
#
# The proof image is rebuilt from scratch (mkfs without
# metadata_csum_seed/orphan_file — the ext4 gate rejects those).
#
# Usage: run/tr4-kvm-proof.sh [bzImage] [logfile-prefix]
# Env:   TIMEOUT_A (default 300), TIMEOUT_ALIAS (default 90)

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
KERNEL="${1:-$ROOT/target/x86_64-unknown-none/release/bzImage}"
PREFIX="${2:-$ROOT/run/tr4-kvm-proof}"
TIMEOUT_A="${TIMEOUT_A:-300}"
TIMEOUT_ALIAS="${TIMEOUT_ALIAS:-90}"
ROOTFS="$ROOT/run/tr4-rootfs.img"
INIT="$ROOT/run/tr4-init"

mkdir -p "$ROOT/run"

# Build the static proof init if missing or stale.
if [ ! -x "$INIT" ] || [ "$ROOT/run/tr4-proof-init.c" -nt "$INIT" ]; then
    gcc -static -O2 -o "$INIT" "$ROOT/run/tr4-proof-init.c" || exit 1
fi

# Build the proof rootfs (a small private image — never a shared one).
if [ ! -f "$ROOTFS" ] || [ "$INIT" -nt "$ROOTFS" ]; then
    rm -f "$ROOTFS"
    truncate -s 256M "$ROOTFS"
    mkfs.ext4 -q -F -O ^metadata_csum_seed,^orphan_file "$ROOTFS" || exit 1
    debugfs -w -R "write $INIT /init" "$ROOTFS" >/dev/null 2>&1 || exit 1
fi

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
        -smp 8 \
        -display none \
        -serial stdio \
        -snapshot \
        -drive file="$ROOTFS",if=none,id=rootfs,format=raw,file.locking=off \
        -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
        -netdev user,id=n0 \
        -device virtio-net-pci,netdev=n0 \
        -kernel "$KERNEL" \
        -append "root=/dev/vda rw init=/init console=ttyS0 $extra_append" \
        > "$log" 2>&1
    echo "serial log: $log"
}

boot "$PREFIX-serial.log" "$TIMEOUT_A" ""
grep -n "===P\|AVAIL10\|LOCKDBG\|NETDBG\|PIPDBG\|PROOF-INIT-DONE\|KERNPANIC\|panic" \
    "$PREFIX-serial.log" | head -30

boot "$PREFIX-alias-serial.log" "$TIMEOUT_ALIAS" "debug=syncguard"
echo "--- alias boot (debug=syncguard): P0 shows the lock family pre-armed ---"
sed -n '/\[P0\] boot state/,/^\[P1\]/p' "$PREFIX-alias-serial.log" | head -14
grep -n "debug: runtime switch\|PROOF-INIT-DONE\|KERNPANIC\|panic" \
    "$PREFIX-alias-serial.log" | head -6

boot "$PREFIX-alias2-serial.log" "$TIMEOUT_ALIAS" "debug=tcptrace"
echo "--- alias boot (debug=tcptrace): P0 shows tcp:+sock: pre-armed ---"
sed -n '/\[P0\] boot state/,/^\[P1\]/p' "$PREFIX-alias2-serial.log" | head -14
grep -n "debug: runtime switch\|PROOF-INIT-DONE\|KERNPANIC\|panic" \
    "$PREFIX-alias2-serial.log" | head -6
