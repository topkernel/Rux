#!/bin/bash
# lb-mkimage2.sh — build run/system-lb2.img (private; never the central
# store): the hmdfs-fixed lb image with the installs service RESIDENT.
#
# Why (lb34, fifth successor): the post-unlock wall is AMS's isBoot
# BMS query ladder ("waiting query highest priority ability info
# completed" x588, nanosleep poll) waiting for CreateNewUser(100)'s
# per-user install stream, which runs through the installd SA host
# ("installs", SA 511). Every installd load on the on-demand path
# costs 15-101 s on Rux ("Start dynamic proc:installs,511,105_*ms" —
# samgr's ServiceControlWithExtra timing out against init), and samgr
# re-loads it repeatedly. Flipping "ondemand" to false starts the
# process once in init's boot stream; samgr then finds SA 511
# registered and the load path never runs.
set -euo pipefail
cd "$(dirname "$0")/.."

SRC="run/system-lb.img"
IMG="run/system-lb2.img"
CFG_TMP=$(mktemp /tmp/installs.cfg.XXXXXX)

if [ ! -f "$IMG" ]; then
    echo "[lb2-image] reflink copy $SRC -> $IMG"
    cp --reflink=auto "$SRC" "$IMG"
fi

debugfs -R "cat /etc/init/installs.cfg" "$IMG" 2>/dev/null > "$CFG_TMP"
grep -q '"ondemand" : true' "$CFG_TMP" || {
    echo "[lb2-image] already resident (or unexpected cfg):"
    grep -E 'ondemand|name' "$CFG_TMP" || true
    rm -f "$CFG_TMP"
    exit 0
}
sed -i 's/"ondemand" : true/"ondemand" : false/' "$CFG_TMP"
printf 'rm /etc/init/installs.cfg\ncd /etc/init\nwrite %s installs.cfg\n' "$CFG_TMP" \
    | debugfs -w "$IMG" >/dev/null 2>&1
debugfs -R "cat /etc/init/installs.cfg" "$IMG" 2>/dev/null | grep -E '"ondemand"|"name"'
rm -f "$CFG_TMP"
