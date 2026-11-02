#!/bin/bash
# repro-nftw.sh — boot the probe image with chunk.txt=nftw01 and capture the
# FULL KERNPANIC dump (the scan driver kills QEMU 2s after seeing "kernel
# panic", truncating the dump; this runner lets it drain to the 180s timeout).
# Usage: bash run/repro-nftw.sh [testname]
set -u
cd /home/william/rux-agents/ltp-r21
T="${1:-nftw01}"
IMG=run/rootfs-x86_64-probe.img
KERN=target/x86_64-unknown-none/debug/bzImage
LOG="run/repro-${T}.log"

printf '%s\n' "$T" > run/chunk-one.txt
DBFS=$(mktemp /tmp/dbfs-XXXX)
printf 'cd /test/linux-ltp\nrm chunk.txt\nwrite %s/run/chunk-one.txt chunk.txt\n' \
    /home/william/rux-agents/ltp-r21 > "$DBFS"
debugfs -w -f "$DBFS" "$IMG" 2>&1 | grep -v '^debugfs' | head -2
rm -f "$DBFS"

timeout 180 qemu-system-x86_64 -M q35 -accel kvm -cpu max \
  -m 2G -smp 2 -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -kernel "$KERN" \
  -append "root=/dev/vda rw init=/test/ltpscan console=ttyS0" \
  > "$LOG" 2>&1 || true
echo "=== $LOG (panic section) ==="
grep -n "PANIC\|Call Trace\|stack words\|rip\|RIP\|Location\|RES|" "$LOG" | head -30
