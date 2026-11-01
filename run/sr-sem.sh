#!/bin/bash
# sr-sem.sh — 2M-round semop ping-pong on THIS worktree's release kernel
# (the fix-lostpark proof rig shape: init=/test/semprobe, q35 KVM -smp 2).
# Reuses the fix-lostpark probe image read-only via -snapshot.
set -u
WT=/home/william/rux-agents/sync-realign
SRC=/home/william/rux-agents/fix-lostpark
LOG="$WT/run/sr-sem-last.log"
rm -f "$LOG"
timeout --signal=TERM --kill-after=5 150 qemu-system-x86_64 \
    -M q35 -accel kvm -cpu host -m 2G -smp 2 \
    -display none -vga none -serial "file:$LOG" -monitor none \
    -kernel "$WT/target/x86_64-unknown-none/release/bzImage" \
    -append "root=/dev/vda rw init=/test/semprobe console=ttyS0" \
    -drive file="$SRC/work/probe.img",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
    -device virtio-blk-pci,drive=rootfs \
    -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
if tr -d '\r' < "$LOG" | grep -aq "PROBE done" \
    && ! tr -d '\r' < "$LOG" | grep -aq "HANG\|KERNPANIC\|panic"; then
    echo "SEM OK"
    tr -d '\r' < "$LOG" | grep -a "PROBE done"
else
    echo "SEM FAIL"
    tail -20 "$LOG"
fi
