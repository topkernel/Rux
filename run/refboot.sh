#!/bin/bash
# ext4-corrupt lane: REFERENCE kernel boot on OUR cooked userdata (the
# why-slow A/B protocol — wsref class). One boot, KVM q35 SMP8 8G,
# snapshot everywhere (the cooked image is read-only evidence), 240s.
# Usage: refboot.sh <tag> <cooked-img>
cd /home/william/rux-agents/ext4-corrupt
set -u
TAG="${1:?tag}"
UD="$2"
SECS=240
AVAIL=$(LC_ALL=C free -m | awk '/^Mem:/{print $7}')
echo "=== $TAG refboot start $(date +%H:%M:%S) avail=${AVAIL}M ==="
if [ "$AVAIL" -lt 10240 ]; then
  while [ "$(LC_ALL=C free -m | awk '/^Mem:/{print $7}')" -lt 10240 ]; do sleep 60; done
fi
# NOTE: NOSNAP unset — snapshot=on everywhere; the reference only READS
# (plus journal replay — which is part of the verification).
USERDATA="$UD" SMP=8 MEM=8G LOGLEVEL=4 \
  python3 run/lb-boot-qmp.py "$TAG" "$SECS" run/oh-img/system.img run/bzImage-ref \
  > "run/${TAG}-driver.out" 2>&1
rc=$?
echo "=== $TAG refboot rc=$rc $(date +%H:%M:%S) ==="
S="run/oh-${TAG}-serial.log"
echo "  lines: $(wc -l < "$S")  panics: $(grep -ac 'KERNPANIC\|kernel panic' "$S")"
echo "  EXT4-fs-error rows: $(grep -aic 'EXT4-fs error\|ext4-fs error' "$S")"
grep -ai 'EXT4-fs error\|extent tree corrupted\|bit already cleared\|Corrupt inode\|bad entry' "$S" | head -10
rm -f run/oh-"${TAG}"-shot-*.ppm
