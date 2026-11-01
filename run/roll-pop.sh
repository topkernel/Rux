#!/bin/bash
# roll-pop.sh — population roller (eleventh successor, 2026-11-01 ~21:40).
# Each boot gets a FRESH REFLINK COPY of userdata-warm.img and boots NOSNAP
# (writes persist into the copy). The lb65 shape (CreateNewUser + bundle
# size:NN + per-user install stream) is the win: when 'bundle size' appears,
# the copy that ran it is GOLDEN — stop, keep it, the operator snapshot-boots
# it for the launcher ladder.
#
# Why per-boot copies: snapshot boots discard the install stream's records,
# and the original warm image's osaccount DB may or may not have user 100
# (lb92 proved the read coin lands); a NOSNAP boot that runs CreateNewUser
# persists BOTH user 100 and the per-user bundle records.
#
# Success detectors (stop, keep copy + VM RUNNING):
#   'bundle size'            — the golden population
#   'SwitchToUser'           — deep pass; keep running for the ladder
# Fail detectors (kill, delete copy, next boot):
#   'userId not exist'       — Shape B
#   quiet serial > 420s / no CreateNewUser by 980s / stuck 1500s
# Only PIDs/groups started by this script are killed.
cd /home/william/rux-agents/oh-launcher || exit 1
MAX=${1:-10}
START_TAG=${2:-100}
KERN=(run/bzImage-ladder-pollfix2 run/bzImage-ladder-fieldpoll)
mkdir -p run/roll-logs
for i in $(seq 0 $((MAX-1))); do
  TAG="lb$((START_TAG+i))"
  K="${KERN[$((i % 2))]}"
  DATA="run/rollpop-${TAG}.img"
  SER="run/oh-${TAG}-serial.log"
  cp --reflink=auto run/userdata-warm.img "$DATA" || { echo "[pop] reflink copy failed for $TAG" | tee -a run/roll-logs/poproller.log; continue; }
  echo "[pop] boot $((i+1))/$MAX tag=$TAG kernel=$K $(date +%H:%M:%S)" | tee -a run/roll-logs/poproller.log
  setsid env NOSNAP=1 USERDATA="$DATA" SMP=2 python3 run/lb-boot.py "$TAG" 2400 run/system-lb3.img "$K" " dfx=ams46,txnhunt" > "run/${TAG}-driver.out" 2>&1 &
  DRV=$!
  sleep 12
  QPID=$(pgrep -f "lb-boot.py $TAG " | head -1)
  QPID=$(pgrep -P "$QPID" 2>/dev/null | head -1)
  echo "[pop] $TAG driver=$DRV qemu=$QPID" | tee -a run/roll-logs/poproller.log
  T0=$(date +%s)
  VERDICT=timeout
  while true; do
    sleep 30
    NOW=$(date +%s)
    EL=$((NOW - T0))
    if ! kill -0 "$DRV" 2>/dev/null; then VERDICT=died; break; fi
    if grep -q 'bundle size' "$SER" 2>/dev/null; then VERDICT=GOLDEN-bundle; break; fi
    if grep -q 'SwitchToUser' "$SER" 2>/dev/null; then VERDICT=SUCCESS-switch; break; fi
    if grep -q 'bootevent.launcher.ready' "$SER" 2>/dev/null; then VERDICT=SUCCESS-ready; break; fi
    if grep -q 'appspawn start com.ohos.launcher\|AppSpawn start com.ohos.launcher' "$SER" 2>/dev/null; then VERDICT=SUCCESS-spawn; break; fi
    if grep -q 'userId not exist' "$SER" 2>/dev/null; then VERDICT=fail-shapeB; break; fi
    M=$(stat -c %Y "$SER" 2>/dev/null || echo "$NOW")
    if [ $((NOW - M)) -gt 420 ]; then VERDICT=fail-quiet; break; fi
    if [ $EL -gt 980 ] && ! grep -q 'CreateNewUser' "$SER" 2>/dev/null; then VERDICT=fail-noprovision; break; fi
    if [ $EL -gt 1600 ]; then VERDICT=fail-stuck; break; fi
  done
  echo "[pop] $TAG verdict=$VERDICT at ${EL}s $(date +%H:%M:%S)" | tee -a run/roll-logs/poproller.log
  case "$VERDICT" in
    GOLDEN-*|SUCCESS-*)
      echo "[pop] $VERDICT — VM left running ($TAG), data=$DATA — OPERATOR TAKE OVER" | tee -a run/roll-logs/poproller.log
      exit 0 ;;
    *)
      kill -- -"$DRV" 2>/dev/null; kill "$DRV" 2>/dev/null
      [ -n "$QPID" ] && kill "$QPID" 2>/dev/null
      sleep 3
      pkill -f "lb-boot.py $TAG " 2>/dev/null
      sleep 2
      rm -f "$DATA"
      ;;
  esac
done
echo "[pop] exhausted $MAX boots, no golden population" | tee -a run/roll-logs/poproller.log
