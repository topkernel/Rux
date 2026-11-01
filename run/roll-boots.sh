#!/bin/bash
# roll-boots.sh — lb lottery roller (eleventh successor, 2026-11-01).
# Boots system-lb3.img + userdata-warm.img with the poll-fix kernels,
# alternating bzImage-ladder-fieldpoll / bzImage-ladder-pollfix2,
# watches the serial log for the known fail shapes, kills and relaunches.
# Success shapes stop the roller (leave the VM RUNNING for the operator).
#
# Fail detectors (kill + next boot):
#   - 'userId not exist'            (Shape B: accountmgr answered 32/false)
#   - serial mtime older than 420s  (console-dead wedge)
#   - 980s elapsed, no CreateNewUser (never reached provisioning)
# Success detectors (stop roller, VM keeps running):
#   - 'bundle size'  (CreateNewUser install stream — ladder live)
#   - 'SwitchToUser' / 'appspawn' + 'com.ohos.launcher' / launcher.ready
#   - screen md5 change is checked by the OPERATOR via the driver shots.
# Only PIDs started by this script are killed. One VM at a time.
cd /home/william/rux-agents/oh-launcher || exit 1
MAX=${1:-8}
START_TAG=${2:-80}
KERN=(run/bzImage-ladder-fieldpoll run/bzImage-ladder-pollfix2)
mkdir -p run/roll-logs
for i in $(seq 0 $((MAX-1))); do
  TAG="lb$((START_TAG+i))"
  K="${KERN[$((i % 2))]}"
  SER="run/oh-${TAG}-serial.log"
  echo "[roll] boot $((i+1))/$MAX tag=$TAG kernel=$K $(date +%H:%M:%S)" | tee -a run/roll-logs/roller.log
  setsid env USERDATA=run/userdata-warm.img SMP=2 python3 run/lb-boot.py "$TAG" 2400 run/system-lb3.img "$K" " dfx=ams46,txnhunt" > "run/${TAG}-driver.out" 2>&1 &
  DRV=$!
  # find the qemu child pid
  sleep 12
  QPID=$(pgrep -f "lb-boot.py $TAG " | head -1)
  QPID=$(pgrep -P "$QPID" 2>/dev/null | head -1)
  [ -z "$QPID" ] && QPID=$(pgrep -f "oh-${TAG}-mon.sock" | head -1)
  echo "[roll] $TAG driver=$DRV qemu=$QPID" | tee -a run/roll-logs/roller.log
  T0=$(date +%s)
  VERDICT=timeout
  while true; do
    sleep 30
    NOW=$(date +%s)
    EL=$((NOW - T0))
    # died on its own?
    if ! kill -0 "$DRV" 2>/dev/null; then VERDICT=died; break; fi
    if grep -q 'bundle size' "$SER" 2>/dev/null; then VERDICT=SUCCESS-bundle; break; fi
    if grep -q 'SwitchToUser' "$SER" 2>/dev/null; then VERDICT=SUCCESS-switch; break; fi
    if grep -q 'com.ohos.launcher' "$SER" 2>/dev/null && grep -q 'appspawn' "$SER" 2>/dev/null; then VERDICT=SUCCESS-spawn; break; fi
    if grep -q 'bootevent.launcher.ready' "$SER" 2>/dev/null; then VERDICT=SUCCESS-ready; break; fi
    if grep -q 'userId not exist' "$SER" 2>/dev/null; then VERDICT=fail-shapeB; break; fi
    M=$(stat -c %Y "$SER" 2>/dev/null || echo "$NOW")
    if [ $((NOW - M)) -gt 420 ]; then VERDICT=fail-quiet; break; fi
    if [ $EL -gt 980 ] && ! grep -q 'CreateNewUser' "$SER" 2>/dev/null; then VERDICT=fail-noprovision; break; fi
    if [ $EL -gt 1500 ]; then VERDICT=fail-stuck; break; fi
  done
  echo "[roll] $TAG verdict=$VERDICT at ${EL}s $(date +%H:%M:%S)" | tee -a run/roll-logs/roller.log
  case "$VERDICT" in
    SUCCESS-*) echo "[roll] SUCCESS — VM left running ($TAG), operator take over" | tee -a run/roll-logs/roller.log; exit 0 ;;
  esac
  kill -- -"$DRV" 2>/dev/null; kill "$DRV" 2>/dev/null
  [ -n "$QPID" ] && kill "$QPID" 2>/dev/null; [ -n "$QPID" ] && kill -- -"$QPID" 2>/dev/null
  sleep 3
  pkill -f "lb-boot.py $TAG " 2>/dev/null
  sleep 2
done
echo "[roll] exhausted $MAX boots, no success" | tee -a run/roll-logs/roller.log
