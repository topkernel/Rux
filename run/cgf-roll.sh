#!/bin/bash
# cgf-roll.sh — comprehensive-fix verification roller (fresh-boot dice).
# Boots the cgf1 kernel (same-proc binder rejection + eventfd poll_wait)
# on FRESH userdata reflinks, self-classifies each boot, kills only ITS
# OWN QEMUs (unique userdata-cgf-N path substring). Modeled on poproll2b.
# Verdicts: WEDGE (SA inserts frozen 240s+ with serial stalled young),
# SPIRAL (ExecReboot / Kernel panic — NOT '/proc/sysrq-trigger' chown
# noise, the v1 roller bug that murdered cgf1/cgf2 at 15s), LAUNCHER
# milestones, full-window. Counts BINDER-REJECT / SYNCSEND-GUARD lines.
cd /home/william/rux-agents/oh-launcher || exit 1
MAX=${1:-8}
START=${2:-1}
KERNEL=${KERNEL:-run/bzImage-cgf1-dbg}
BASE=${BASE:-run/userdata-fresh2.img}
LOG=run/roll-logs/cgf-roll.log
mkdir -p run/roll-logs

kill_my_qemu() {  # $1 = unique substring (this boot's userdata path)
  local UD="$1" p c
  for p in /proc/[0-9]*/cmdline; do
    c=$(tr '\0' ' ' < "$p" 2>/dev/null) || continue
    case "$c" in
      qemu-system-x86_64*"$UD"*) kill -9 "${p//[!0-9]/}" 2>/dev/null ;;
    esac
  done
  for i in 1 2 3 4 5 6; do
    local alive=0
    for p in /proc/[0-9]*/cmdline; do
      c=$(tr '\0' ' ' < "$p" 2>/dev/null) || continue
      case "$c" in qemu-system-x86_64*"$UD"*) alive=1 ;; esac
    done
    [ "$alive" -eq 0 ] && return 0
    sleep 2
  done
  return 1
}

cnt() { grep -ac "$1" "$2" 2>/dev/null | head -1; [ $? -eq 0 ] || echo 0; }

classify() {  # $1 = serial log
  local S="$1" SA NOW TXN
  SA=$(cnt 'insert SA:' "$S")
  NOW=$(grep -a 'insert SA:' "$S" 2>/dev/null | tail -1 | grep -oP '^\[\s*\K[0-9.]+')
  TXN=$(cnt 'SLOW-TXN' "$S")
  if grep -aq 'bundle size' "$S" 2>/dev/null; then echo "BUNDLE-SIZE(sA=$SA)";
  elif grep -aq 'SwitchToUser' "$S" 2>/dev/null; then echo "SWITCH(sA=$SA)";
  elif grep -aq 'startUser in service' "$S" 2>/dev/null; then echo "STARTUSER(sA=$SA)";
  elif grep -aqE 'ExecReboot|Kernel panic' "$S" 2>/dev/null; then echo "SPIRAL(sA=$SA)";
  elif [ "${SA:-0}" -ge 40 ]; then echo "HEALTHY-noladder(sA=$SA)";
  else echo "EARLY(sA=$SA txn=$TXN last@$NOW)"; fi
}

for i in $(seq $START $((START+MAX-1))); do
  TAG="cgf$i"
  SER="run/oh-${TAG}-serial.log"
  UD="run/userdata-cgf-$i.img"
  cp --reflink=auto "$BASE" "$UD" 2>/dev/null || cp "$BASE" "$UD"
  echo "[cgf] boot $TAG $(date +%H:%M:%S) kernel=$KERNEL load=$(cut -d' ' -f1 /proc/loadavg)" >> "$LOG"
  setsid env SMP=2 LOGLEVEL=6 USERDATA="$UD" \
    python3 run/lb-boot.py "$TAG" 1800 run/system-lb5.img "$KERNEL" \
    " dfx=ams46,txnhunt negmemo=1" > "run/${TAG}-driver.out" 2>&1 &
  DRV=$!
  sleep 10
  T0=$(date +%s); VERDICT=running; SAPREV=0; STALLT=0
  while true; do
    sleep 15
    EL=$(( $(date +%s) - T0 ))
    kill -0 "$DRV" 2>/dev/null || { VERDICT=ended; break; }
    if grep -aq 'appspawn.*com.ohos.launcher' "$SER" 2>/dev/null; then VERDICT=LAUNCHER-SPAWN; break; fi
    if grep -aq 'Handle set param.*launcher\.ready' "$SER" 2>/dev/null; then VERDICT=launcher-ready; break; fi
    if grep -aq 'bundle size' "$SER" 2>/dev/null; then VERDICT=bundle-size; break; fi
    if grep -aqE 'ExecReboot|Kernel panic' "$SER" 2>/dev/null; then VERDICT=spiral; break; fi
    # wedge detect v2 (cgf1 lesson): SA inserts frozen 240s+ while young
    # is the wedge — the fdreap/RCU-GP stall spam keeps the serial bytes
    # flowing, so serial-quiet must NOT gate this arm. Serial-quiet alone
    # (no SA, no spam) is the dark-boot face — keep it as a second arm.
    SANOW=$(cnt 'insert SA:' "$SER")
    if [ "${SANOW:-0}" -gt "$SAPREV" ]; then SAPREV=$SANOW; STALLT=0; else STALLT=$((STALLT+15)); fi
    SZ=$(stat -c %s "$SER" 2>/dev/null || echo 0); sleep 8; SZ2=$(stat -c %s "$SER" 2>/dev/null || echo 0)
    if [ "$EL" -lt 500 ] && [ "$STALLT" -ge 240 ] && [ "${SANOW:-0}" -lt 70 ]; then
      VERDICT=wedge; break
    fi
    if [ "$SZ" = "$SZ2" ] && [ "$EL" -lt 500 ] && [ "$STALLT" -ge 240 ]; then
      VERDICT=wedge-dark; break
    fi
    [ "$EL" -ge 1500 ] && { VERDICT=full-window; break; }
  done
  CLS=$(classify "$SER")
  REJ=$(cnt 'BINDER-REJECT' "$SER")
  REJCTX=$(cnt 'ctxmgr-own-process' "$SER")
  REJSELF=$(cnt 'self-transaction' "$SER")
  GUARD=$(cnt 'SYNCSEND-GUARD' "$SER")
  echo "[cgf] $TAG verdict=$VERDICT class=$CLS rejects=$REJ(ctx=$REJCTX,self=$REJSELF) guard=$GUARD el=${EL}s $(date +%H:%M:%S)" >> "$LOG"
  kill_my_qemu "userdata-cgf-$i"
  pkill -f "lb-boot.py $TAG " 2>/dev/null
  sleep 5
done
echo "[cgf] ROLL COMPLETE $(date)" >> "$LOG"
