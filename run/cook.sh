#!/bin/bash
# ext4-corrupt lane: cook one userdata image with OUR kernel, then e2fsck.
# Protocol identical to why-slow cook4-6: blank mkfs 16G ext4, NOSNAP boot
# (writes persist), 300s window, then e2fsck -n -f on the cooked bytes.
# Usage: cook.sh <tag> [secs]
cd /home/william/rux-agents/ext4-corrupt
set -u
TAG="${1:?tag}"
SECS="${2:-300}"
KERNEL="target/x86_64-unknown-none/debug/bzImage"
UD="run/ud-${TAG}.img"

# Blank cook image: same geometry as userdata-f16g-fresh3. NOTE: this
# host's mke2fs 1.47.2 default profile now enables orphan_file +
# metadata_csum_seed (c7 evidence: mount refused ENODEV, /data never
# populated) — the fresh3 lineage has neither. Disable both explicitly.
truncate -s 16G "$UD"
mke2fs -q -t ext4 -O ^orphan_file,^metadata_csum_seed -F "$UD"
dumpe2fs -h "$UD" 2>/dev/null | grep -q orphan_file && { echo "FATAL: orphan_file still on"; exit 1; }

AVAIL=$(LC_ALL=C free -m | awk '/^Mem:/{print $7}')
echo "=== $TAG cook start $(date +%H:%M:%S) avail=${AVAIL}M secs=$SECS load='$(cat /proc/loadavg)' ==="
if [ "$AVAIL" -lt 10240 ]; then
  echo "avail mem ${AVAIL}MB < 10G — waiting"
  while [ "$(LC_ALL=C free -m | awk '/^Mem:/{print $7}')" -lt 10240 ]; do sleep 60; done
fi
USERDATA="$UD" SMP=8 MEM=8G LOGLEVEL=4 NOSNAP=1 \
  python3 run/lb-boot-qmp.py "$TAG" "$SECS" run/oh-img/system.img "$KERNEL" \
  > "run/${TAG}-driver.out" 2>&1
rc=$?
echo "=== $TAG boot rc=$rc $(date +%H:%M:%S) ==="
S="run/oh-${TAG}-serial.log"
echo "  lines: $(wc -l < "$S")  panics: $(grep -ac 'KERNPANIC\|kernel panic' "$S" 2>/dev/null)"
grep -a 'launcher.ready\|bootanimation.finished' "$S" | tail -2
# Keep the cooked image (unlike ws-boot.sh which deletes it) — this lane's
# whole point is post-mortem on the bytes. Serial stays for reference.
e2fsck -n -f "$UD" > "run/${TAG}-e2fsck.out" 2>&1
echo "e2fsck rc=$?"
tail -3 "run/${TAG}-e2fsck.out"
rm -f run/oh-"${TAG}"-shot-*.ppm
