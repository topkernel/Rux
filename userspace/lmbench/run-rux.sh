#!/bin/bash
# run-rux.sh — one boot per lmbench case on the Rux x86_64 kernel
# (KVM). Prints "<case>|<measurement lines>".
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WT="$(cd "$HERE/../.." && pwd)"
KERNEL="${1:-$WT/target/x86_64-unknown-none/release/bzImage}"
IMG="${2:-$WT/work/lmbench-x86.img}"
OUT="${3:-$WT/run/lmbench-rux.txt}"
TIMEOUT="${4:-45}"
: > "$OUT"
while read -r spec; do
    [ -n "$spec" ] || continue
    name="${spec%% *}"
    log="/tmp/lm-$name-$$.log"
    timeout "$TIMEOUT" qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial "file:$log" -monitor none \
        -kernel "$KERNEL" \
        -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$(echo $spec | tr ' ' ',') console=ttyS0" \
        -drive file="$IMG",if=none,id=rootfs,format=raw \
        -device virtio-blk-pci,drive=rootfs \
        -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
    echo "===CASE $spec" >> "$OUT"
    tr -d '\r' < "$log" 2>/dev/null | grep -aE "[0-9]+\.[0-9]+|LMBENCH (TIMEOUT|EXEC-FAIL)" >> "$OUT"
    rm -f "$log"
done < "$HERE/bench-list.txt"
echo "run-rux: results -> $OUT"
