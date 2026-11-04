#!/bin/bash
# cg32-diag.sh — 32nd successor, mission-2 diagnostic boot: the stress
# lane's lying-slot instruments (dfx=lostpark: WAKE-SKIP-ORIGIN names the
# slot's last writer at the why=c refusal; PARK-REFUSED-ORIGIN names the
# preempt-class holdout) on a populated-store boot that wedges. One boot,
# no verdict pressure: watch to SA-freeze+360s (past the wedge onset, so
# the lostpark census + TXN-HANG rescans collect evidence through the
# wedge), then kill + decode.
TAG=${1:-lb421}
SER="run/oh-${TAG}-serial.log"
UD="run/userdata-cg32diag.img"
KERNEL=${KERNEL:-run/bzImage-fullfix2-dbg}
cd /home/william/rux-agents/oh-launcher || exit 1
cp --reflink=auto run/bms-ref.img "$UD" 2>/dev/null || cp run/bms-ref.img "$UD"
echo "[diag] boot $TAG $(date +%H:%M:%S) flags=ams46,txnhunt,lostpark"
setsid env NOSNAP=1 SMP=2 LOGLEVEL=6 USERDATA="$UD" \
  python3 run/lb-boot.py "$TAG" 1500 run/system-lb5.img "$KERNEL" \
  " dfx=ams46,txnhunt,lostpark negmemo=1" > "run/${TAG}-driver.out" 2>&1 &
DRV=$!
sleep 10
T0=$(date +%s); SAPREV=0; SAT=0
while true; do
  sleep 15
  EL=$(( $(date +%s) - T0 ))
  kill -0 "$DRV" 2>/dev/null || break
  SA=$(grep -a -c 'insert SA:' "$SER" 2>/dev/null)
  [ "$SA" -gt "$SAPREV" ] && { SAPREV=$SA; SAT=$EL; }
  echo "[diag] el=${EL}s SA=$SA"
  # collect 360s of post-wedge evidence, then stop
  if [ $((EL - SAT)) -gt 360 ]; then echo "[diag] wedge window collected"; break; fi
  if [ $EL -gt 1400 ]; then break; fi
done
pkill -f "lb-boot.py $TAG " 2>/dev/null
for p in /proc/[0-9]*/cmdline; do
  c=$(tr '\0' ' ' < "$p" 2>/dev/null) || continue
  case "$c" in qemu-system-x86_64*"$UD"*) kill -9 "${p//[!0-9]/}" 2>/dev/null ;; esac
done
wait $DRV 2>/dev/null
echo "[diag] done — decode: WAKE-SKIP-ORIGIN / PARK-REFUSED / LP- census"
