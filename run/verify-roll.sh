#!/bin/bash
# verify-roll.sh — measure the store-load window (bundles.ready) on the
# biofix kernel, unmodified image (system-lb5, snapshot boots on warm data).
# Re-rolls past samgr-dark boots (SA inserts == 0 by 300s cannot reach BMS).
cd /home/william/rux-agents/oh-launcher || exit 1
MAX=${1:-6}
START=${2:-233}
for i in $(seq 0 $((MAX-1))); do
  TAG="lb$((START+i))"
  SER="run/oh-${TAG}-serial.log"
  echo "[verify] boot $TAG $(date +%H:%M:%S)" >> run/roll-logs/verify.log
  setsid env SMP=2 LOGLEVEL=6 \
    python3 run/lb-boot.py "$TAG" 1500 run/system-lb5.img run/bzImage-ladder-fp-biofix \
    " dfx=ams46,txnhunt" > "run/${TAG}-driver.out" 2>&1 &
  DRV=$!
  sleep 10
  T0=$(date +%s); VERDICT=timeout
  while true; do
    sleep 5
    EL=$(( $(date +%s) - T0 ))
    kill -0 "$DRV" 2>/dev/null || { VERDICT=died; break; }
    if grep -aq 'bootevent.bms.main.bundles.ready' "$SER" 2>/dev/null; then VERDICT=READY; break; fi
    SA=$(grep -a -c 'insert SA:' "$SER" 2>/dev/null)
    if [ "$SA" -eq 0 ] && [ $EL -gt 300 ]; then VERDICT=dark; break; fi
    M=$(stat -c %Y "$SER" 2>/dev/null || echo "$(date +%s)")
    if [ $(( $(date +%s) - M )) -gt 420 ]; then VERDICT=quiet; break; fi
    if [ $EL -gt 1450 ]; then VERDICT=timeout; break; fi
  done
  T=$(grep -a -m1 -o '\[ *[0-9]*\.[0-9]*\][^(]*bootevent\.bms\.main\.bundles\.ready' "$SER" 2>/dev/null | grep -o '^.*\]' | head -1)
  echo "[verify] $TAG verdict=$VERDICT ready='$T' at ${EL}s" >> run/roll-logs/verify.log
  if [ "$VERDICT" = READY ]; then
    echo "$TAG" > run/roll-logs/VERIFY-HIT
    exit 0
  fi
  kill -- -"$DRV" 2>/dev/null; kill "$DRV" 2>/dev/null
  sleep 3
  for p in $(pgrep -f "lb-boot.py $TAG " 2>/dev/null); do kill "$p" 2>/dev/null; done
  sleep 2
done
echo "[verify] exhausted" >> run/roll-logs/verify.log
