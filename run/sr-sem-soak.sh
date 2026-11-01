#!/bin/bash
# sr-sem-soak.sh <cycles> — repeated 2M ping-pong probe boots on this
# worktree's release kernel (the fix-lostpark soak shape). Every cycle
# must print PROBE done; any HANG/panic fails.
set -u
WT=/home/william/rux-agents/sync-realign
SRC=/home/william/rux-agents/fix-lostpark
N="${1:-25}"
OK=0; BAD=0; C=0
while [ $C -lt $N ]; do
    C=$((C+1))
    LOG="$WT/run/sr-soak-$C.log"
    rm -f "$LOG"
    timeout --signal=TERM --kill-after=5 90 qemu-system-x86_64 \
        -M q35 -accel kvm -cpu host -m 2G -smp 2 \
        -display none -vga none -serial "file:$LOG" -monitor none \
        -kernel "$WT/target/x86_64-unknown-none/release/bzImage" \
        -append "root=/dev/vda rw init=/test/semprobe console=ttyS0" \
        -drive file="$SRC/work/probe.img",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs \
        -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
    if tr -d '\r' < "$LOG" 2>/dev/null | grep -aq "PROBE done" \
        && ! tr -d '\r' < "$LOG" 2>/dev/null | grep -aq "HANG\|KERNPANIC\|panic"; then
        OK=$((OK+1)); echo "cycle $C OK: $(tr -d '\r' < "$LOG" | grep -a 'PROBE done' | head -1)"
    else
        BAD=$((BAD+1)); cp "$LOG" "$WT/run/sr-soak-FAIL-$C.log"; echo "cycle $C FAIL"
    fi
    rm -f "$LOG"
done
echo "SEM-SOAK cycles=$C ok=$OK bad=$BAD"
