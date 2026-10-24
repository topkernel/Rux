#!/bin/bash
# pw-oh-boots.sh — sequential OH-shape KVM boots (printk-wedge verify).
#
# The b140 wedge hit 1-in-5 KVM boots at ~3.5 min, so each round runs
# 420s (SMP2, 4G, q35, KVM — the decisive-round shape) and the pass
# criterion is: no "BUG: soft lockup", serial still advancing at TERM,
# boot reaching the same late milestone as known-good b1xx logs.
#
# Usage: pw-oh-boots.sh [count]
set -u
ROOT=/home/william/rux-agents/printk-wedge
cd "$ROOT"
N=${1:-6}
SECS=${SECS:-420}
for i in $(seq 1 "$N"); do
    echo "=== OH boot $i/$N start $(date +%H:%M:%S) ==="
    SMP=2 ACCEL=kvm SECS=$SECS ./run/oh-boot.sh "pw-oh$i" \
        || echo "boot $i driver exit=$?"
    echo "=== OH boot $i/$N done  $(date +%H:%M:%S) ==="
done
echo "=== all $N OH boots finished $(date +%H:%M:%S) ==="
