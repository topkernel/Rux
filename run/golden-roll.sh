#!/bin/bash
# golden-roll.sh — thirteenth successor: deterministic golden-population roller.
# Recipe per boot: FRESH sparse copy of userdata-warm.img (populated pre-install
# KVS + poisoned user-100) + system-lb6.img (sdc stub strips /data account dir
# at 4.3s -> user 100 absent) + bzImage-ladder-gpbound, NOSNAP.
# Creation fires when accountmgr sees storage(5003)+AMS(180)+BMS(401) SAs
# registered (source: AccountMgrService::OnAddSystemAbility), then reads the
# POPULATED store -> 'bundle size:34' -> per-user install stream persists ->
# GOLDEN. Boot-to-boot lottery: samgr-dark boots (no SA inserts) and stall
# chains (hdf/power) are duds; kill fast and re-roll.
cd /home/william/rux-agents/oh-launcher || exit 1
MAX=${1:-8}
START=${2:-212}
KERN=run/bzImage-ladder-gpbound
IMG=run/system-lb8.img
BASE=run/userdata-warm.img
mkdir -p run/roll-logs
for i in $(seq 0 $((MAX-1))); do
  TAG="lb$((START+i))"
  DATA="run/golden-${TAG}.img"
  SER="run/oh-${TAG}-serial.log"
  cp --sparse=always "$BASE" "$DATA" || { echo "[gold] copy failed $TAG" >> run/roll-logs/goldroll.log; continue; }
  echo "[gold] boot $((i+1))/$MAX tag=$TAG $(date +%H:%M:%S)" >> run/roll-logs/goldroll.log
  setsid env NOSNAP=1 USERDATA="$DATA" SMP=2 LOGLEVEL=6 \
    python3 run/lb-boot.py "$TAG" 2400 "$IMG" "$KERN" " dfx=ams46,txnhunt" \
    > "run/${TAG}-driver.out" 2>&1 &
  DRV=$!
  sleep 10
  QPID=$(pgrep -f "^python3 run/lb-boot.py $TAG " | head -1)
  QPID=$(pgrep -P "$QPID" 2>/dev/null | head -1)
  T0=$(date +%s); VERDICT=timeout; PHASE=early
  while true; do
    sleep 5
    EL=$(( $(date +%s) - T0 ))
    kill -0 "$DRV" 2>/dev/null || { VERDICT=died; break; }
    if grep -aq 'bundle size' "$SER" 2>/dev/null; then VERDICT=GOLDEN; break; fi
    if grep -aqE 'appspawn start com.ohos.launcher|AppSpawn start com.ohos.launcher' "$SER" 2>/dev/null; then VERDICT=SUCCESS-spawn; break; fi
    if grep -aq 'Handle set param.*launcher.ready' "$SER" 2>/dev/null; then VERDICT=SUCCESS-ready; break; fi
    SA=$(grep -a -c 'insert SA:' "$SER" 2>/dev/null)
    if [ "$PHASE" = early ]; then
      if [ "$SA" -gt 5 ]; then PHASE=flowing; echo "[gold] $TAG SA flowing (${SA} @${EL}s)" >> run/roll-logs/goldroll.log
      elif [ $EL -gt 150 ]; then VERDICT=dud-samgr-dark; break; fi
    fi
    if grep -aq 'SwitchToUser' "$SER" 2>/dev/null && [ "$VERDICT" = timeout ]; then VERDICT=SUCCESS-switch; break; fi
    if grep -aq 'bundle size' "$SER" 2>/dev/null; then VERDICT=GOLDEN; break; fi
    M=$(stat -c %Y "$SER" 2>/dev/null || echo "$(date +%s)")
    if [ $(( $(date +%s) - M )) -gt 420 ]; then VERDICT=stall-quiet; break; fi
    if [ $EL -gt 2200 ]; then VERDICT=timeout; break; fi
  done
  echo "[gold] $TAG verdict=$VERDICT at ${EL}s $(date +%H:%M:%S)" >> run/roll-logs/goldroll.log
  case "$VERDICT" in
    GOLDEN|SUCCESS-*)
      echo "$VERDICT $TAG $DATA" > "run/roll-logs/TAKEOVER-$TAG"
      echo "[gold] $VERDICT — VM LEFT RUNNING — OPERATOR TAKE OVER ($DATA)" >> run/roll-logs/goldroll.log
      exit 0 ;;
    *)
      kill -- -"$DRV" 2>/dev/null; kill "$DRV" 2>/dev/null; [ -n "$QPID" ] && kill "$QPID" 2>/dev/null
      sleep 4
      # belt-and-braces: kill any leftover qemu whose cmdline names this boot's data img (by pid, never pkill)
      for p in $(pgrep -f "golden-${TAG}.img" 2>/dev/null); do kill "$p" 2>/dev/null; done
      sleep 2
      rm -f "$DATA" ;;
  esac
done
echo "[gold] exhausted $MAX boots" >> run/roll-logs/goldroll.log
