#!/bin/bash
# lp-soak.sh <minutes> — repeated 2M ping-pong probe boots for the lost-park
# soak. Every cycle must print PROBE done; any HANG/panic fails the soak.
set -u
WT=/home/william/rux-agents/fix-lostpark
MINS="${1:-30}"
END=$((SECONDS + MINS * 60))
CYCLE=0
OK=0
BAD=0
while [ $SECONDS -lt $END ]; do
    CYCLE=$((CYCLE + 1))
    LOG="$WT/run/lp-soak-$CYCLE.log"
    rm -f "$LOG"
    timeout --signal=TERM --kill-after=5 90 qemu-system-x86_64 \
        -M q35 -accel kvm -cpu host -m 2G -smp 2 \
        -display none -vga none -serial "file:$LOG" -monitor none \
        -kernel "$WT/target/x86_64-unknown-none/release/bzImage" \
        -append "root=/dev/vda rw init=/test/semprobe console=ttyS0" \
        -drive file="$WT/work/probe.img",if=none,id=rootfs,format=raw,snapshot=on,file.locking=off \
        -device virtio-blk-pci,drive=rootfs \
        -device virtio-net-pci,netdev=n0 -netdev user,id=n0 >/dev/null 2>&1
    if tr -d '\r' < "$LOG" 2>/dev/null | grep -aq "PROBE done" \
        && ! tr -d '\r' < "$LOG" 2>/dev/null | grep -aq "HANG\|KERNPANIC\|panic"; then
        OK=$((OK + 1))
    else
        BAD=$((BAD + 1))
        cp "$LOG" "$WT/run/lp-soak-FAIL-$CYCLE.log"
    fi
    rm -f "$LOG"
done
echo "SOAK cycles=$CYCLE ok=$OK bad=$BAD"
