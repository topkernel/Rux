#!/bin/bash
# ft03-ab.sh — ftest03 isolation A/B (LTP r24 handoff): N boots per kernel,
# debug builds, private stress rootfs, KVM. Verdicts from the child TFAIL
# lines (DATA checks) per boot.
# Usage: ft03-ab.sh <kernel> <tag> <nboots> [extra-append]
set -u
cd "$(dirname "$0")/.."
KERN="$1"; TAG="$2"; N="${3:-2}"; APPEND="${4:-}"
IMG=run/rootfs-stress.img
mkdir -p run/stress-runs
for i in $(seq 1 "$N"); do
  LOG=run/stress-runs/ft03-${TAG}-${i}.log
  printf 'ftest03\n' > /tmp/ft03-chunk.txt
  debugfs -w -f /dev/stdin "$IMG" >/dev/null 2>&1 <<DBFS
cd /test/linux-ltp
rm chunk.txt
write /tmp/ft03-chunk.txt chunk.txt
DBFS
  timeout 300 qemu-system-x86_64 -M q35 -accel kvm -cpu max \
    -m 2G -smp 2 -display none -serial stdio -monitor none -no-reboot \
    -snapshot -drive file="$IMG",if=none,id=rootfs,format=raw,file.locking=off \
    -device virtio-blk-pci,drive=rootfs \
    -kernel "$KERN" \
    -append "root=/dev/vda rw init=/test/ltpscan console=ttyS0 $APPEND" \
    < /dev/null > "$LOG" 2>&1
  RC=$?
  VERDICT=$(grep -a -c "TFAIL" "$LOG" 2>/dev/null || true)
  BAD=$(grep -a -m2 -oE "bad verify @ 0x[0-9a-f]+ (for val [0-9]+ )?(count [0-9]+)?" "$LOG" | head -2 | tr '\n' ';')
  RES=$(grep -a -m1 "RES|ftest03" "$LOG" | cut -d'|' -f3-4 | tr -d '\r')
  echo "BOOT $TAG-$i rc=$RC tfails=$VERDICT res='$RES' badverify='$BAD'"
done
