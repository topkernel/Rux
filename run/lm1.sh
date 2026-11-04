#!/bin/bash
# lm1.sh <case-spec> [kernel] [tag] [timeout] — one boot per case, keep serial.
# case-spec uses commas for spaces (cmdline parser drops quotes).
set -u
WT=/home/william/rux-agents/lmbench-fix
KERNEL="${2:-$WT/target/x86_64-unknown-none/debug/bzImage}"
IMG=/home/william/rux-agents/perf-lmbench/work/lmbench-x86.img
TAG="${3:-x}"
TMO="${4:-70}"
SPEC="$1"
NAME="$(echo "$SPEC" | tr ',/' '__')"
LOG="$WT/run/lm1-${TAG}-${NAME}.log"
rm -f "$LOG"
timeout "$TMO" qemu-system-x86_64 -M q35 -accel kvm -cpu host \
    -m 2G -smp 2 -display none -vga none -serial "file:$LOG" -monitor none \
    -kernel "$KERNEL" \
    -append "root=/dev/vda rw init=/test/lmbench_run lmbench=$SPEC console=ttyS0" \
    -drive file="$IMG",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
    -device virtio-blk-pci,drive=rootfs \
    -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
echo "=== $SPEC [$TAG] -> $LOG"
grep -aE "TCP latency|UDP latency|Pipe latency|connection latency|Socket bandwidth|Semaphore|LMBENCH (TIMEOUT|EXEC-FAIL|DONE)|Recv timed" "$LOG" | grep -av RCU-GP | tail -4
