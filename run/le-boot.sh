#!/bin/bash
# le-boot.sh (launcher-errno lane): ONE boot, ours or reference, private
# reflink userdata copy, KVM q35 SMP8 8G, LOGLEVEL=4, NOSNAP first-boot
# population (the why-slow ws-boot.sh proven flow). Usage:
#   le-boot.sh <tag> <kernel-path> [extra-append] [secs]
cd /home/william/rux-agents/launcher-errno
set -u
TAG="${1:?tag}"
KERNEL="${2:?kernel path}"
EXTRA="${3:-}"
SECS="${4:-420}"
UD="run/ud-${TAG}.img"
rm -f "$UD"
cp --reflink=auto "${VEHICLE:-run/userdata-fresh3.img}" "$UD"
AVAIL=$(LC_ALL=C free -m | awk '/^Mem:/{print $7}')
echo "=== $TAG start $(date +%H:%M:%S) avail=${AVAIL}M kernel=$KERNEL secs=$SECS extra='$EXTRA' load='$(cat /proc/loadavg)' ==="
if [ "$AVAIL" -lt 10240 ]; then
  echo "avail mem ${AVAIL}MB < 10G — waiting"
  while [ "$(LC_ALL=C free -m | awk '/^Mem:/{print $7}')" -lt 10240 ]; do sleep 60; done
fi
USERDATA="$UD" SMP=8 MEM=8G LOGLEVEL=${LOGLEVEL:-4} NOSNAP=1 \
  python3 run/lb-boot-qmp.py "$TAG" "$SECS" run/oh-img/system.img "$KERNEL" "$EXTRA" \
  > "run/${TAG}-driver.out" 2>&1
rc=$?
echo "=== $TAG rc=$rc $(date +%H:%M:%S) load='$(cat /proc/loadavg)' ==="
S="run/oh-${TAG}-serial.log"
echo "  lines: $(wc -l < "$S")  panics: $(grep -ac 'KERNPANIC\|kernel panic' "$S" 2>/dev/null)"
cat "run/oh-${TAG}-markers.txt" 2>/dev/null | sed 's/^/    MK /'
gzip -f "$S"
rm -f run/oh-"${TAG}"-shot-*.ppm "$UD"
