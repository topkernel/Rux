#!/bin/bash
# A/B: hotpath_probe on BEFORE (base, no print-audit fixes) vs AFTER
# (this branch). x86_64 KVM, interleaved boots, 3 runs each.
set -u
WT="/home/william/rux-agents/code-review-perf"
IMG="$WT/run/printaudit/probe.img"
OUT="$WT/run/printaudit/ab-probe-results.txt"
: > "$OUT"
run_case() {
    local kern="$1" tag="$2" iter="$3"
    local log="/tmp/ab-hp-$tag-$iter.log"
    timeout 45 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
        -kernel "$kern" \
        -append "root=/dev/vda rw init=/hotpath_probe console=ttyS0" \
        -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs \
        -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
    if [ "$iter" = 1 ]; then
        echo "--- $tag flood counts (whole boot) ---" >> "$OUT"
        for pat in "MPROTECT-RW" "BIGMUNMAP" "TKSIG" "stkfutex" "FXDV" "FTWAIT"; do
            printf "%s: %s\n" "$pat" "$(tr -d '\r' < "$log" | grep -ac "$pat")" >> "$OUT"
        done
    fi
    echo "=== $tag iter$iter" >> "$OUT"
    tr -d '\r' < "$log" | grep -a "HPROBE" >> "$OUT"
    rm -f "$log"
}
for i in 1 2 3; do
    run_case "$WT/run/printaudit/bzImage-BEFORE" BEFORE $i
    run_case "$WT/run/printaudit/bzImage-AFTER"  AFTER  $i
done
echo done >> "$OUT"
