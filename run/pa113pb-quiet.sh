#!/bin/bash
# pa113pb-quiet.sh — rerun-once quiet-window pbench board for the
# out-of-band I/O cells of the 2026-10-31 pa112pb battery
# (p6_write_fsync med-of-2 17081 vs the main-merge-aarch64 quiet
# family 12155-12432 +38%; p6_append 708 vs 637-649; p6_mkdrmdir
# 24736 at the +19% edge — board2 p99 7.1s shows mid-phase host
# contention; the merge boards ran at foreign_qemus=1, these at 10-11).
# Gate: load1<=6 AND foreign qemus<=8 held for TWO consecutive
# samples (battery window was load1 8-10 / 10-11 qemus; the task's
# own Tier-1 enforcement gate is load1<=9 / qemus<=8 — this sits
# between that and the merge boards' foreign_qemus=1),
# deadline 3h from arm; ONE board, tag pa113pb.
cd "$(dirname "$0")/.." || exit 1
LOG=run/pa113pb-quiet.log
K=run/pa110-main-dbg-bzImage
DEADLINE=$(( $(date +%s) + 3*3600 ))

stamp() { date '+%F %T'; }
l1() { awk '{printf "%d", $1}' /proc/loadavg; }
nq() { ps aux | grep -c "[q]emu-system" || true; }

echo "=== pa113pb quiet rerun armed $(stamp) (K md5=$(md5sum "$K" | cut -c1-12)) gate: load1<=6 qemus<=8 x2 ===" >> "$LOG"
gate_open() { [ "$(l1)" -le 6 ] && [ "$(nq)" -le 8 ]; }
while :; do
    gate_open || { echo "$(stamp) wait: load1=$(l1) qemus=$(nq)" >> "$LOG"; sleep 120; continue; }
    sleep 60   # stability: require the gate to hold for two samples
    gate_open && break
    echo "$(stamp) false dip: load1=$(l1) qemus=$(nq)" >> "$LOG"
done
[ "$(date +%s)" -lt "$DEADLINE" ] || { echo "$(stamp) DEADLINE hit, no quiet window" >> "$LOG"; exit 4; }
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
