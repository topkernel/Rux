#!/bin/bash
# poproll2b.sh — 31st successor, v2 of the populated-store roller (fixes
# poproll2.sh's kill bug: `pgrep -x qemu-system-x86_64` never matches —
# comm is truncated to 15 chars — so the roller hung in `wait` and its
# boots kept running past verdict; two fsck samples got tainted by live
# writers). Kill is now a /proc cmdline scan for THIS boot's userdata
# path, verified dead before proceeding. Adds: post-kill e2fsck of every
# boot's image (the mission's crash-consistency acceptance generalized),
# faster wedge detect (SA frozen 240s + TXN-HANG), serial-quiet stall.
cd /home/william/rux-agents/oh-launcher || exit 1
MAX=${1:-8}
START=${2:-389}
KERNEL=${KERNEL:-run/ccrust/bzImage-fixext-dbg}
BASE=${BASE:-run/bms-ref.img}
LOG=run/roll-logs/poproll31.log
FSCK=${FSCK:-1}
mkdir -p run/roll-logs

kill_my_qemu() {  # $1 = userdata path; scans /proc cmdlines (pgrep -x bug)
  local UD="$1" p c
  for p in /proc/[0-9]*/cmdline; do
    c=$(tr '\0' ' ' < "$p" 2>/dev/null) || continue
    case "$c" in
      qemu-system-x86_64*"$UD"*) kill -9 "${p//[!0-9]/}" 2>/dev/null ;;
    esac
  done
  for i in 1 2 3 4 5 6; do  # verify dead
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

for i in $(seq 0 $((MAX-1))); do
  TAG="lb$((START+i))"
  SER="run/oh-${TAG}-serial.log"
  UD="run/userdata-pop31-$i.img"
  cp --reflink=auto "$BASE" "$UD" 2>/dev/null || cp "$BASE" "$UD"
  echo "[pop2] boot $TAG $(date +%H:%M:%S) kernel=$KERNEL load=$(cut -d' ' -f1 /proc/loadavg)" >> "$LOG"
  setsid env NOSNAP=1 SMP=2 LOGLEVEL=6 USERDATA="$UD" \
    python3 run/lb-boot.py "$TAG" 1800 run/system-lb5.img "$KERNEL" \
    " dfx=${DFX:-ams46,txnhunt} negmemo=1" > "run/${TAG}-driver.out" 2>&1 &
  DRV=$!
  sleep 10
  T0=$(date +%s); VERDICT=timeout; SAPREV=0; SAT=0
  while true; do
    sleep 15
    EL=$(( $(date +%s) - T0 ))
    kill -0 "$DRV" 2>/dev/null || { VERDICT=ended; break; }
    if grep -aq 'userId not exist' "$SER" 2>/dev/null; then VERDICT=fail-shapeB; break; fi
    if grep -aqE 'appspawn.*com.ohos.launcher|AppSpawn.*com.ohos.launcher' "$SER" 2>/dev/null; then VERDICT=LAUNCHER-SPAWN; break; fi
    # lb389 lesson: bare 'launcher.ready' matches init's "not set" warnings —
    # require the POSITIVE set-param form
    if grep -aq 'Handle set param.*launcher\.ready' "$SER" 2>/dev/null; then VERDICT=launcher-ready; break; fi
    if grep -aq 'SwitchToUser' "$SER" 2>/dev/null; then VERDICT=switch; fi
    if grep -aq 'startUser' "$SER" 2>/dev/null && [ "$VERDICT" = timeout ]; then VERDICT=startUser; fi
    if grep -aqE 'KERNPANIC|kernel panic' "$SER" 2>/dev/null; then VERDICT=panic; break; fi
    for P in run/oh-${TAG}-shot-*.ppm; do
      [ -f "$P" ] || continue
      M=$(md5sum "$P" | cut -d' ' -f1 | cut -c1-8)
      case "$M" in
        5f8b704d|06b933b4|82f0a25f|23312e5b) : ;;
        *) echo "[pop2] $TAG PIXEL-CHANGE $P md5=$M" >> "$LOG" ;;
      esac
    done
    SA=$(grep -a -c 'insert SA:' "$SER" 2>/dev/null)
    if [ "$SA" -gt "$SAPREV" ]; then SAPREV=$SA; SAT=$EL; fi
    if [ "$SA" -eq 0 ] && [ $EL -gt 400 ]; then VERDICT=dark; break; fi
    # cg36: signature-early verdict — the comm-inline FUX-STUCK rows name
    # the wedge park's owner thread directly, but the owner PROCESS comes
    # from tgid: samgr's wedge park sits on OS_IPC/FFRT-named threads,
    # not a comm=samgr thread, so resolve samgr's pid from its
    # ServiceStart print and match the row by tgid. Once samgr's
    # +0x800194 park has survived >=90s (one 60s timeout cycle + margin)
    # its u2v caller decode is banked; no need to sit the full 240s
    # wedge window. Records concatenate without newlines on the serial,
    # so the park age must be matched record-bounded (no FUX-STUCK token
    # between the tgid=, park= and +0x800194 anchors) — python re.
    SAMPID=$(grep -ao "ServiceStart started info samgr(pid [0-9]*" "$SER" 2>/dev/null | head -1 | grep -o "[0-9]*$")
    if [ -n "$SAMPID" ] && [ "${SIGKILL:-1}" = "1" ]; then
      SIGAGE=$(python3 -c '
import re,sys
pat=re.compile((r"tgid=%s(?:(?!FUX-STUCK).)*?park=(\d+)s(?:(?!FUX-STUCK).)*?A\+0x800194" % sys.argv[2]).encode(),re.S)
best=0
try:
    with open(sys.argv[1],"rb") as f:
        for m in pat.finditer(f.read()):
            best=max(best,int(m.group(1)))
except OSError: pass
print(best)' "$SER" "$SAMPID" 2>/dev/null)
      [ -z "$SIGAGE" ] && SIGAGE=0
      if [ "$SIGAGE" -ge 90 ]; then VERDICT=samgr-sig; break; fi
    fi
    if [ $((EL - SAT)) -gt 240 ] && grep -aq 'TXN-HANG' "$SER" 2>/dev/null; then VERDICT=wedge; break; fi
    M=$(stat -c %Y "$SER" 2>/dev/null || echo "$(date +%s)")
    if [ $(( $(date +%s) - M )) -gt 240 ]; then VERDICT=quiet-stall; break; fi
    if [ $EL -gt 1750 ]; then break; fi
  done
  SA=$(grep -a -c 'insert SA:' "$SER" 2>/dev/null)
  CU=$(grep -a -c 'CreateNewUser' "$SER" 2>/dev/null)
  QUOTA=$(grep -a -c 'SetBundleQuota' "$SER" 2>/dev/null)
  JLESS=$(grep -a -c 'journal inode empty' "$SER" 2>/dev/null)
  echo "[pop2] $TAG verdict=$VERDICT SA=$SA createNewUser=$CU quota=$QUOTA journal-less=$JLESS el=${EL}s load=$(cut -d' ' -f1 /proc/loadavg)" >> "$LOG"
  case "$VERDICT" in
    LAUNCHER-SPAWN|launcher-ready)
      echo "[pop2] $TAG MILESTONE — VM LEFT RUNNING — pixel watch" >> "$LOG"
      exit 0 ;;
  esac
  pkill -f "lb-boot.py $TAG " 2>/dev/null
  sleep 1
  if kill_my_qemu "$UD"; then
    echo "[pop2] $TAG qemu killed clean" >> "$LOG"
  else
    echo "[pop2] $TAG WARNING qemu still alive after kill attempts" >> "$LOG"
  fi
  wait $DRV 2>/dev/null
  # post-kill acceptance fsck (quiescent image)
  if [ "$FSCK" = 1 ] && [ -f "$UD" ]; then
    e2fsck -fy "$UD" > "run/pop31-${i}-fsck.log" 2>&1
    RC=$?
    CL=$(grep -a -c "修复？是" "run/pop31-${i}-fsck.log")
    echo "[pop2] $TAG post-kill e2fsck rc=$RC fix-lines=$CL ($(basename $UD))" >> "$LOG"
  fi
  sleep 5
done
echo "[pop2] done $(date +%H:%M:%S)" >> "$LOG"
