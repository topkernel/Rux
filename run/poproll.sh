#!/bin/bash
# poproll.sh — population-race roller (14th successor): system-lb5
# (unmodified image) + userdata-warm.img (populated pre-install store,
# no user-100 — lb64's bundle size:34 lineage) + fx4fix kernel
# (biofix + pcp fixes + allocwatch). The race: CreateNewUser(100)'s
# 900s retry budget vs bundles.ready (store-load). Bio fix should
# compress the load so the race wins -> bundle size:34 -> per-user
# install stream -> SwitchToUser -> launcher AbilityInfo -> appspawn
# -> PIXELS.
cd /home/william/rux-agents/oh-launcher || exit 1
MAX=${1:-6}
START=${2:-260}
for i in $(seq 0 $((MAX-1))); do
  TAG="lb$((START+i))"
  SER="run/oh-${TAG}-serial.log"
  echo "[pop] boot $TAG $(date +%H:%M:%S)" >> run/roll-logs/pop14.log
  setsid env SMP=2 LOGLEVEL=6 USERDATA=run/userdata-warm.img \
    python3 run/lb-boot.py "$TAG" 1800 run/system-lb5.img run/bzImage-ladder-fx5fix \
    " dfx=ams46,txnhunt,allocwatch" > "run/${TAG}-driver.out" 2>&1 &
  DRV=$!
  sleep 10
  T0=$(date +%s); VERDICT=timeout
  while true; do
    sleep 10
    EL=$(( $(date +%s) - T0 ))
    kill -0 "$DRV" 2>/dev/null || { VERDICT=died; break; }
    if grep -aq 'appspawn' "$SER" 2>/dev/null && grep -aq 'com.ohos.launcher' "$SER" 2>/dev/null; then
      VERDICT=LAUNCHER-SPAWN; break
    fi
    if grep -aq 'bundle size:0[^0-9]' "$SER" 2>/dev/null; then VERDICT=empty-set; fi
    if grep -aq 'create user:100 bundle size:34' "$SER" 2>/dev/null; then VERDICT=race-won; fi
    if grep -aq 'SwitchToUser' "$SER" 2>/dev/null; then VERDICT=switch; fi
    if grep -aq 'ALLOC-HOLD\|ALLOC-STUCK' "$SER" 2>/dev/null; then VERDICT=alloc-freeze; break; fi
    SA=$(grep -a -c 'insert SA:' "$SER" 2>/dev/null)
    if [ "$SA" -eq 0 ] && [ $EL -gt 300 ]; then VERDICT=dark; break; fi
    M=$(stat -c %Y "$SER" 2>/dev/null || echo "$(date +%s)")
    if [ $(( $(date +%s) - M )) -gt 420 ]; then VERDICT=quiet; break; fi
    if [ $EL -gt 1750 ]; then VERDICT=timeout; break; fi
  done
  R=$(grep -a -m1 -o '\[ *[0-9]*\.[0-9]*\][^(]*create user:100 bundle size:[0-9]*' "$SER" 2>/dev/null | grep -o 'size:[0-9]*')
  B=$(grep -a 'bootevent.bms.main.bundles.ready' "$SER" 2>/dev/null | grep -av 'not set' | head -1 | grep -a -o '\[ *[0-9]*\.[0-9]*\]' | head -1)
  C=$(grep -a -m1 -o '\[ *[0-9]*\.[0-9]*\][^(]*CreateNewUser user(100) start' "$SER" 2>/dev/null | grep -o '\[ *[0-9]*\.[0-9]*\]' | head -1)
  A=$(grep -a -m3 'ALLOC-HOLD\|ALLOC-STUCK' "$SER" 2>/dev/null | head -3)
  echo "[pop] $TAG verdict=$VERDICT create='$C' bundles='$B' $R at ${EL}s" >> run/roll-logs/pop14.log
  [ -n "$A" ] && echo "[pop] $TAG $A" >> run/roll-logs/pop14.log
  if [ "$VERDICT" = LAUNCHER-SPAWN ] || [ "$VERDICT" = switch ]; then
    echo "$TAG" > run/roll-logs/POP-HIT
    exit 0   # VM LEFT RUNNING for the pixel watch
  fi
  kill -- -"$DRV" 2>/dev/null; kill "$DRV" 2>/dev/null
  sleep 3
  for p in $(pgrep -f "lb-boot.py $TAG " 2>/dev/null); do kill "$p" 2>/dev/null; done
  sleep 2
done
echo "[pop] exhausted" >> run/roll-logs/pop14.log
