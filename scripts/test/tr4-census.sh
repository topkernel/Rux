#!/bin/bash
# tr4-census.sh — dfx-trace round 4 guarded-domain pre-verify: a quiet
# census A/B (events off) on the sched/mm/fs guarded domains.
#
# Same boot shape as rounds 2/3 (tr2-census.sh): both kernels, one
# shared userspace (the round-4 static-init image, idle after its
# flow), syscost=1 debug=wwcensus, 480 s each, q35 KVM smp2/4G.
#   A = this branch's x86_64 debug bzImage
#   B = origin/main debug bzImage (dfx-trace4-base worktree)
# Census classes compared side by side: TERM-DESCHED / WAKE-REFUSED /
# WAKE-STRANDED totals, panics / OOM, the syscost tail.

set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
A_KERNEL="$ROOT/target/x86_64-unknown-none/debug/bzImage"
B_KERNEL="/home/william/rux-agents/dfx-trace4-base/target/x86_64-unknown-none/debug/bzImage"
DUR="${DUR:-480}"
IMG="$ROOT/work/tr4-census-rootfs.img"

cp "$ROOT/run/tr4-rootfs.img" "$IMG"

boot() {
    local kernel="$1" log="$2"
    rm -f "$log"
    {
        sleep 3
    } | timeout "$DUR" qemu-system-x86_64 \
        -M q35 -accel kvm -cpu max -m 4G -smp 8 \
        -display none -serial stdio -snapshot \
        -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
        -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
        -kernel "$kernel" \
        -append "root=/dev/vda rw init=/init console=ttyS0 syscost=1 debug=wwcensus" \
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

boot "$A_KERNEL" "$ROOT/run/tr4-census-a.log"
summary "A (this branch)" "$ROOT/run/tr4-census-a.log"
boot "$B_KERNEL" "$ROOT/run/tr4-census-b.log"
summary "B (origin/main)" "$ROOT/run/tr4-census-b.log"

echo "== syscost tail (A then B) =="
tail -4 "$ROOT/run/tr4-census-a.log" | cut -c1-160
echo "---"
tail -4 "$ROOT/run/tr4-census-b.log" | cut -c1-160
