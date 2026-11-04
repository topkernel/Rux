#!/bin/bash
# r30roll.sh — 30th successor race roller. Kernel = current tree debug
# (extent ee_len==0x8000 fix + jbd2 write-through + ams46 BMSSCAN probe).
# Free discriminator per boot (28th notes): BMSSCAN install_list.json
# opens + store-dir mkdirs = install branch FIRED; RDB storm + 40s park
# + zero mkdirs = empty completion. NEW this session: /data mounts must
# NOT print 'journal inode empty, disabled' (extent fix) — bank that
# line's presence/absence per boot. Pixels: md5 leaving the frozen set
# {5f8b704d,06b933b4,82f0a25f,23312e5b} -> convert + report IMMEDIATELY.
cd /home/william/rux-agents/oh-launcher || exit 1
MAX=${1:-6}
START=${2:-380}
KERNEL=${KERNEL:-run/ccrust/bzImage-fixext-dbg}
LOG=run/roll-logs/r30roll.log
mkdir -p run/roll-logs
for i in $(seq 0 $((MAX-1))); do
  TAG="lb$((START+i))"
  SER="run/oh-${TAG}-serial.log"
  UD="run/userdata-r30-$i.img"
  cp --reflink=auto run/userdata-fresh2.img "$UD" 2>/dev/null || cp run/userdata-fresh2.img "$UD"
  echo "[r30] boot $TAG $(date +%H:%M:%S) kernel=$KERNEL load=$(cut -d' ' -f1 /proc/loadavg)" >> "$LOG"
  setsid env SMP=2 LOGLEVEL=6 USERDATA="$UD" \
    python3 run/lb-boot.py "$TAG" 1800 run/system-lb5.img "$KERNEL" \
    " dfx=ams46,txnhunt negmemo=1" > "run/${TAG}-driver.out" 2>&1 &
  DRV=$!
  sleep 10
  T0=$(date +%s); VERDICT=timeout; SAPREV=0; SAT=0
  while true; do
    sleep 15
    EL=$(( $(date +%s) - T0 ))
    kill -0 "$DRV" 2>/dev/null || { VERDICT=ended; break; }
    # branch FIRED: the install leg's fs actions (mkdir on bms chain or
    # install_list.json open) — the free discriminator
    if grep -aq 'BMSSCAN-MKDIR.*bundle_manager_service' "$SER" 2>/dev/null \
       || grep -aq 'BMSSCAN-OPEN.*install_list' "$SER" 2>/dev/null; then VERDICT=branch-FIRED; fi
    if grep -aq 'create user:100 bundle size:34' "$SER" 2>/dev/null; then VERDICT=race-won; break; fi
    if grep -aq 'appspawn' "$SER" 2>/dev/null && grep -aq 'com.ohos.launcher' "$SER" 2>/dev/null; then VERDICT=LAUNCHER-SPAWN; break; fi
    if grep -aqE 'KERNPANIC|kernel panic' "$SER" 2>/dev/null; then VERDICT=panic; break; fi
    SA=$(grep -a -c 'insert SA:' "$SER" 2>/dev/null)
    # wedge detect (32nd: ported from poproll2b — wedged fresh boots
    # otherwise burn the full 1900s watch for no extra information)
    if [ "$SA" -gt "$SAPREV" ]; then SAPREV=$SA; SAT=$EL; fi
    if [ "$SA" -eq 0 ] && [ $EL -gt 400 ]; then VERDICT=dark; break; fi
    if [ $((EL - SAT)) -gt 240 ] && grep -aq 'TXN-HANG' "$SER" 2>/dev/null; then VERDICT=wedge; break; fi
    M=$(stat -c %Y "$SER" 2>/dev/null || echo "$(date +%s)")
    if [ $(( $(date +%s) - M )) -gt 240 ]; then VERDICT=quiet-stall; break; fi
    if [ $EL -gt 1900 ]; then break; fi
  done
  # park + empty-completion face (no early break needed)
  PARK=$(grep -a -c 'install_list.json' "$SER" 2>/dev/null)
  JLESS=$(grep -a -c 'journal inode empty' "$SER" 2>/dev/null)
  echo "[r30] $TAG verdict=$VERDICT install_list_lines=$PARK journal-less-lines=$JLESS el=${EL}s" >> "$LOG"
  # pixel md5 sweep of this boot's screendumps
  for P in run/oh-${TAG}-shot-*.ppm; do
    [ -f "$P" ] || continue
    M=$(md5sum "$P" | cut -d' ' -f1 | cut -c1-8)
    case "$M" in
      5f8b704d|06b933b4|82f0a25f|23312e5b) : ;;
      *) echo "[r30] $TAG PIXEL-CHANGE $P md5=$M" >> "$LOG" ;;
    esac
  done
  # only kill OUR qemu (this boot's, by disk path match). /proc cmdline
  # scan: pgrep -x never matches (comm truncated to 15 chars, the name is
  # 17) — the same bug the 31st fixed in poproll2b.
  for p in /proc/[0-9]*/cmdline; do
    c=$(tr '\0' ' ' < "$p" 2>/dev/null) || continue
    case "$c" in
      qemu-system-x86_64*"$UD"*) kill -9 "${p//[!0-9]/}" 2>/dev/null ;;
    esac
  done
  for w in 1 2 3 4 5 6; do  # verify dead before next boot
    alive=0
    for p in /proc/[0-9]*/cmdline; do
      c=$(tr '\0' ' ' < "$p" 2>/dev/null) || continue
      case "$c" in qemu-system-x86_64*"$UD"*) alive=1 ;; esac
    done
    [ "$alive" -eq 0 ] && break
    sleep 2
  done
  wait $DRV 2>/dev/null
  sleep 5
done
echo "[r30] done $(date +%H:%M:%S)" >> "$LOG"
