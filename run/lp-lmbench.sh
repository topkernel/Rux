#!/bin/bash
# lp-lmbench.sh <case...> — one boot per lmbench case at -smp 2 KVM with
# the fix kernel. Prints the measurement lines.
set -u
WT=/home/william/rux-agents/fix-lostpark
KERNEL="$WT/target/x86_64-unknown-none/release/bzImage"
IMG="$WT/work/lmbench-x86.img"
for CASE in "$@"; do
    LOG="$WT/run/lp-lm-$CASE.log"
    SPEC="$(echo "$CASE" | tr ' ' ',')"
    rm -f "$LOG"
    timeout 90 qemu-system-x86_64 -M q35 -accel kvm -cpu host \
        -m 2G -smp 2 -display none -vga none -serial "file:$LOG" -monitor none \
        -kernel "$KERNEL" \
        -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$SPEC console=ttyS0" \
        -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs \
        -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
    echo "=== $CASE"
    tr -d '\r' < "$LOG" | grep -aE "latency|Latency|throughput|[0-9]+\.[0-9]+ ?(microsecond|milliseconds|MB)|LMBENCH (TIMEOUT|EXEC-FAIL)|Semaphore|TCP|connection" | tail -6
done
