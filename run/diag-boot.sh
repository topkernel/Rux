#!/bin/bash
# diag-boot.sh — boot the probe image with init=/bin/sh, feed commands on
# serial stdin, capture FULL test output to run/diag-<tag>.log.
# Usage: bash run/diag-boot.sh <cmdfile> <tag> [timeout_s]
set -u
cd /home/william/rux-agents/ltp-r21
CMD="${1:?cmdfile}"; TAG="${2:?tag}"; TMO="${3:-600}"
IMG=run/rootfs-x86_64-probe.img
KERN=target/x86_64-unknown-none/debug/bzImage
LOG="run/diag-${TAG}.log"
timeout "$TMO" sh -c "qemu-system-x86_64 -M q35 -accel kvm -cpu max \
  -m 2G -smp 2 -display none -serial stdio -monitor none -no-reboot -snapshot \
  -drive file=\"$IMG\",if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -kernel \"$KERN\" \
  -append \"root=/dev/vda rw init=/bin/sh console=ttyS0\" < \"$CMD\"" > "$LOG" 2>&1 || true
echo "=== $LOG written ==="
