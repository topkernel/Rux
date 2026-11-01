#!/bin/bash
# repro.sh <case-spec-with-commas> [timeout] — one boot, RAW serial kept
# (the sweep driver deletes serial logs; this one keeps them so the
# functional-gap evidence is preserved).
WT=/home/william/rux-agents/perf-lmbench
SPEC="$1"; TMO="${2:-70}"
LOG="$WT/run/repro-$(echo "$SPEC" | tr ',/' '__').log"
timeout "$TMO" qemu-system-x86_64 -M q35 -accel kvm -cpu host \
    -m 2G -smp 2 -display none -vga none -serial "file:$LOG" -monitor none \
    -kernel "$WT/work/bzImage-lm" \
    -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$SPEC console=ttyS0" \
    -drive file="$WT/work/lmbench-x86.img",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
    -device virtio-blk-pci,drive=rootfs \
    -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
echo "=== serial ($LOG):"
tr -d '\r' < "$LOG" | tail -25
