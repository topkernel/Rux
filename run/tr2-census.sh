#!/bin/bash
# tr2-census.sh — dfx-trace round 2 guarded-domain pre-verify: a quiet
# census A/B (events off) on the sched/mm guarded domains.
#
# The standard Tier-1 vehicle (pa-boot.sh 780 s, tok109 userdata) is
# the perf-audit lane's merge-gate asset; the lane-level pre-verify
# here runs the SAME boot shape on both kernels with one shared
# userspace (the round-2 static-init image (run/tr2-rootfs.img), idle after its flow) and
# compares the census classes side by side:
#   - TERM-DESCHED / WAKE-REFUSED / WAKE-STRANDED totals
#   - panics / OOM classes
#   - the syscost dump tail (tick + volume classes)
#
# A = this branch's x86_64 debug bzImage; B = origin/main d2b2672a
# debug bzImage (built in /tmp/rux-main-tree with the same make build
# sequence). 480 s each, q35 KVM smp2/4G, syscost=1 dfx=wwcensus.

set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
A_KERNEL="$ROOT/target/x86_64-unknown-none/debug/bzImage"
B_KERNEL="/tmp/rux-main-tree/target/x86_64-unknown-none/debug/bzImage"
DUR="${DUR:-480}"
IMG="$ROOT/work/tr2-census-rootfs.img"

cp "$ROOT/run/tr2-rootfs.img" "$IMG"

boot() {
    local kernel="$1" log="$2"
    rm -f "$log"
    {
        sleep 3
    } | timeout "$DUR" qemu-system-x86_64 \
        -M q35 -accel kvm -cpu max -m 4G -smp 2 \
        -display none -serial stdio -snapshot \
        -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
        -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
        -kernel "$kernel" \
        -append "root=/dev/vda rw init=/init console=ttyS0 syscost=1 dfx=wwcensus" \
        > "$log" 2>&1
}

summary() {
    local tag="$1" log="$2"
    echo "== $tag =="
    for pat in "TERM-DESCHED" "WAKE-REFUSED" "WAKE-STRANDED" "KERNPANIC" "panic" "out of memory" "OOM"; do
        printf "  %-16s %s\n" "$pat:" "$(grep -ac "$pat" "$log" 2>/dev/null || echo 0)"
    done
    echo "  syscost lines:    $(grep -ac "SYSCOST" "$log" 2>/dev/null || echo 0)"
}

boot "$A_KERNEL" "$ROOT/run/tr2-census-a.log"
summary "A (this branch)" "$ROOT/run/tr2-census-a.log"
boot "$B_KERNEL" "$ROOT/run/tr2-census-b.log"
summary "B (origin/main)" "$ROOT/run/tr2-census-b.log"

echo "== syscost tail (A then B) =="
tail -4 "$ROOT/run/tr2-census-a.log" | cut -c1-160
echo "---"
tail -4 "$ROOT/run/tr2-census-b.log" | cut -c1-160
