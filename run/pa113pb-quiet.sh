#!/bin/bash
# pa113pb-quiet.sh — rerun-once quiet-window pbench board for the
# out-of-band I/O cells of the 2026-10-31 pa112pb battery
# (p6_write_fsync med-of-2 17081 vs the main-merge-aarch64 quiet
# family 12155-12432 +38%; p6_append 708 vs 637-649; p6_mkdrmdir
# 24736 at the +19% edge — board2 p99 7.1s shows mid-phase host
# contention; the merge boards ran at foreign_qemus=1, these at 10-11).
# Gate: load1<=5 AND foreign qemus<=4 (pa-boot Tier-1-class quiet),
# deadline 3h from arm; ONE board, tag pa113pb.
cd "$(dirname "$0")/.." || exit 1
LOG=run/pa113pb-quiet.log
K=run/pa110-main-dbg-bzImage
DEADLINE=$(( $(date +%s) + 3*3600 ))

stamp() { date '+%F %T'; }
l1() { awk '{printf "%d", $1}' /proc/loadavg; }
nq() { ps aux | grep -c "[q]emu-system" || true; }

echo "=== pa113pb quiet rerun armed $(stamp) (K md5=$(md5sum "$K" | cut -c1-12)) gate: load1<=5 qemus<=4 ===" >> "$LOG"
while [ "$(l1)" -gt 5 ] || [ "$(nq)" -gt 4 ]; do
    echo "$(stamp) wait: load1=$(l1) qemus=$(nq)" >> "$LOG"
    [ "$(date +%s)" -ge "$DEADLINE" ] && { echo "$(stamp) DEADLINE hit, no quiet window" >> "$LOG"; exit 4; }
    sleep 120
done
echo "--- quiet window OPEN $(stamp) load1=$(l1) qemus=$(nq): boarding ---" >> "$LOG"
python3 test/pbench_boot.py --bzimage "$K" --tag pa113pb --timeout 1200 \
    > run/pa113pb-boot.log 2>&1
rc=$?
echo "board rc=$rc ($(stamp)) load1=$(l1) qemus=$(nq)" >> "$LOG"
{
    echo "--- TABLE pa113pb ($(stamp)) ---"
    grep "PBENCH PREP\|PBENCH p6_\|PBENCH DONE" "work/pbench-pa113pb-serial.log" 2>/dev/null
    echo "panics=$(grep -c 'KERNPANIC' work/pbench-pa113pb-serial.log 2>/dev/null)"
} >> "$LOG"
echo "=== pa113pb quiet rerun complete $(stamp) ===" >> "$LOG"
