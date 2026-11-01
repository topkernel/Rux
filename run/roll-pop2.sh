#!/bin/bash
# roll-pop2.sh — thirteenth successor population roller (2026-11-02).
#
# Stage-1 hunt: fresh sparse copy of userdata-fresh2.img per boot, NOSNAP,
# system-lb5.img (resident installs + sdc stub) + bzImage-ladder-gpbound
# (all crawl fixes: chunked munmap, RCU-bounded exit, self-send, comm_str).
#
# Verdicts per boot:
#   GOLDEN        'bundle size' anywhere        -> leave VM RUNNING, flag, exit
#   SUCCESS-*     SwitchToUser / launcher spawn -> leave VM RUNNING, flag, exit
#   fail-order    CreateNewUser/GenerateUserKeys BEFORE bundles.ready
#                 -> kill FAST (protect copy), delete copy
#   stage1        bundles.ready landed, no poison -> keep watching same boot;
#                 on later kill/quiet/timeout KEEP copy as stage-1 asset and
#                 re-boot it next round (populated store + user absent =>
#                 creation-after-ready becomes near-deterministic)
#   fail-shapeB   'userId not exist'            -> kill, delete
#   fail-quiet    serial quiet >420s            -> kill (keep if stage1)
#   fail-noready  no bundles.ready by 1500s     -> kill, delete
# ESCAPE RULE: after ESCAPE_N=2 consecutive fail-order boots, the next
# creation-before-ready is ALLOWED TO RUN THROUGH (kill only after ready+120s)
# -> produces a scanned-poisoned copy (warm-equivalent) named escape-<tag>.img
# for the image-side user-100 strip hunt.
# Only PIDs started by this script are killed.
cd /home/william/rux-agents/oh-launcher || exit 1
MAX=${1:-6}
START_TAG=${2:-200}
ESCAPE_N=2
KERN=run/bzImage-ladder-gpbound
IMG=run/system-lb5.img
BASE=run/userdata-fresh2.img
mkdir -p run/roll-logs
ORDER_FAILS=0
for i in $(seq 0 $((MAX-1))); do
  TAG="lb$((START_TAG+i))"
  # stage-2: if a stage-1 copy exists, boot it instead of a fresh one
  DATA=$(ls run/stage1-*.img 2>/dev/null | head -1)
  if [ -n "$DATA" ]; then
    MODE=stage2
  else
    MODE=stage1
    DATA="run/goldpop2-${TAG}.img"
    cp --reflink=auto --sparse=always "$BASE" "$DATA" || { echo "[pop2] copy failed $TAG" | tee -a run/roll-logs/poproller2.log; continue; }
  fi
  SER="run/oh-${TAG}-serial.log"
  echo "[pop2] boot $((i+1))/$MAX tag=$TAG mode=$MODE data=$DATA kernel=$KERN $(date +%H:%M:%S)" | tee -a run/roll-logs/poproller2.log
  setsid env NOSNAP=1 USERDATA="$DATA" SMP=2 LOGLEVEL=6 \
    python3 run/lb-boot.py "$TAG" 2400 "$IMG" "$KERN" " dfx=ams46,txnhunt" \
    > "run/${TAG}-driver.out" 2>&1 &
  DRV=$!
  sleep 10
  QPID=$(pgrep -f "^python3 run/lb-boot.py $TAG " | head -1)
  QPID=$(pgrep -P "$QPID" 2>/dev/null | head -1)
  echo "[pop2] $TAG driver=$DRV qemu=$QPID" | tee -a run/roll-logs/poproller2.log
  T0=$(date +%s); VERDICT=timeout; READY=0; CREATION=0
  while true; do
    sleep 2
    NOW=$(date +%s); EL=$((NOW - T0))
    if ! kill -0 "$DRV" 2>/dev/null; then VERDICT=died; break; fi
    if grep -aq 'bundle size' "$SER" 2>/dev/null; then VERDICT=GOLDEN-bundle; break; fi
    if grep -aq 'appspawn start com.ohos.launcher\|AppSpawn start com.ohos.launcher\|bootevent.launcher.ready' "$SER" 2>/dev/null; then VERDICT=SUCCESS-spawn; break; fi
    if grep -aq 'SwitchToUser' "$SER" 2>/dev/null; then VERDICT=SUCCESS-switch; break; fi
    if grep -aq 'userId not exist' "$SER" 2>/dev/null; then VERDICT=fail-shapeB; break; fi
    if [ "$READY" = 0 ] && grep -aq 'bootevent.bms.main.bundles.ready' "$SER" 2>/dev/null; then
      READY=1; echo "[pop2] $TAG bundles.ready SET at ${EL}s" | tee -a run/roll-logs/poproller2.log
    fi
    if [ "$CREATION" = 0 ] && grep -aqE 'CreateNewUser|GenerateUserKeys' "$SER" 2>/dev/null; then
      CREATION=1
      if [ "$READY" = 1 ] || [ "$MODE" = stage2 ]; then
        echo "[pop2] $TAG creation ($MODE, ready=$READY) at ${EL}s — riding to golden" | tee -a run/roll-logs/poproller2.log
      elif [ "$ORDER_FAILS" -ge "$ESCAPE_N" ]; then
        echo "[pop2] $TAG ESCAPE boot: creation-before-ready allowed to run through" | tee -a run/roll-logs/poproller2.log
      else
        VERDICT=fail-order; break
      fi
    fi
    M=$(stat -c %Y "$SER" 2>/dev/null || echo "$NOW")
    if [ $((NOW - M)) -gt 420 ]; then VERDICT=fail-quiet; break; fi
    if [ "$READY" = 0 ] && [ $EL -gt 1500 ]; then VERDICT=fail-noready; break; fi
    if [ $EL -gt 2300 ]; then VERDICT=timeout; break; fi
  done
  if [ "$VERDICT" = fail-order ]; then ORDER_FAILS=$((ORDER_FAILS+1)); fi
  # escape asset: creation-before-ready ran THROUGH and ready later landed
  if [ "$CREATION" = 1 ] && [ "$READY" = 1 ] && [ "$MODE" = stage1 ] \
     && [ "$VERDICT" != GOLDEN-bundle ] && [ "$VERDICT" != SUCCESS-switch ] && [ "$VERDICT" != SUCCESS-spawn ]; then
    VERDICT="escape-scanned"
  fi
  if [ "$READY" = 1 ] && [ "$CREATION" = 0 ] && [ "$VERDICT" != GOLDEN-bundle ] && [ "$VERDICT" != SUCCESS-switch ] && [ "$VERDICT" != SUCCESS-spawn ]; then VERDICT="$VERDICT+stage1"; fi
  echo "[pop2] $TAG verdict=$VERDICT at ${EL}s $(date +%H:%M:%S)" | tee -a run/roll-logs/poproller2.log
  case "$VERDICT" in
    GOLDEN-*|SUCCESS-*)
      echo "[pop2] $VERDICT — VM LEFT RUNNING ($TAG), data=$DATA — write run/roll-logs/TAKEOVER-$TAG" | tee -a run/roll-logs/poproller2.log
      echo "$VERDICT $TAG $DATA" > "run/roll-logs/TAKEOVER-$TAG"
      exit 0 ;;
    *stage1)
      kill -- -"$DRV" 2>/dev/null; kill "$DRV" 2>/dev/null; [ -n "$QPID" ] && kill "$QPID" 2>/dev/null
      sleep 3
      if [ "$MODE" = stage1 ]; then mv "$DATA" "run/stage1-${TAG}.img"; echo "[pop2] kept stage-1 copy run/stage1-${TAG}.img" | tee -a run/roll-logs/poproller2.log; fi ;;
    escape-scanned)
      kill -- -"$DRV" 2>/dev/null; kill "$DRV" 2>/dev/null; [ -n "$QPID" ] && kill "$QPID" 2>/dev/null
      sleep 3
      mv "$DATA" "run/escape-${TAG}.img"; echo "[pop2] kept escape (scanned+poisoned) copy run/escape-${TAG}.img" | tee -a run/roll-logs/poproller2.log ;;
    *)
      kill -- -"$DRV" 2>/dev/null; kill "$DRV" 2>/dev/null; [ -n "$QPID" ] && kill "$QPID" 2>/dev/null
      sleep 3
      if [ "$MODE" = stage1 ]; then rm -f "$DATA"; else rm -f "$DATA"; fi ;;
  esac
done
echo "[pop2] exhausted $MAX boots" | tee -a run/roll-logs/poproller2.log
