#!/bin/bash
# r20-gate.sh — relaunch the r20 scan when the host quiets down.
# Gate: 5-min loadavg < 9 AND fewer than 12 QEMUs. Re-checks every 10 min,
# gives up after 8 h. One launch, then exits (the scan driver handles its
# own wedge retries). Log: .ltp/scan-r20-gate.log
cd "$(dirname "$0")/.."
DEADLINE=$(( $(date +%s) + 8*3600 ))
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
  LOAD=$(awk '{print int($2)}' /proc/loadavg)          # 5-min load, integer part
  NQ=$(ps aux | grep -c '[q]emu-system-riscv64')
  echo "$(date +%H:%M) gate: load=$LOAD qemus=$NQ"
  if [ "$LOAD" -lt 9 ] && [ "$NQ" -lt 12 ]; then
    echo "$(date +%H:%M) gate: quiet — launching r20 scan"
    setsid python3 test/ltpscan/scan.py \
      --kernel .ltp/tgt-r20/riscv64gc-unknown-none-elf/debug/rux \
      --tests test/ltpscan/tests-all.txt \
      --out .ltp/results-r20.txt > .ltp/scan-r20.log 2>&1
    echo "$(date +%H:%M) gate: scan exited rc=$?"
    exit 0
  fi
  sleep 600
done
echo "$(date +%H:%M) gate: 8h window expired without a quiet host — r20 not run"
exit 1
