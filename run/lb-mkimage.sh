#!/bin/bash
# lb-mkimage.sh — build run/system-lb.img (private; never the central store):
# the GL-fixed oh-svc system image with the distributed-file property OFF so
# storage_daemon takes the LocalMount path (bind mounts) instead of mounting
# the "hmdfs" filesystem Rux does not implement. Without this flip,
# StorageDaemonProvider::StartUser fails (mount fstype hmdfs -> EINVAL),
# UnlockUser ends with errCode 13600721, osaccount never calls AMS
# StartUser(100), and the launcher never starts (the Ext/ABCM retry loop
# then hammers GetBundleInfoV9 with userId 0 forever).
set -euo pipefail
cd "$(dirname "$0")/.."

SRC="run/oh-img/system.img"
IMG="run/system-lb.img"
PARA_TMP=$(mktemp /tmp/dfp.para.XXXXXX)

if [ ! -f "$IMG" ]; then
    echo "[lb-image] reflink copy $SRC -> $IMG"
    cp --reflink=auto "$SRC" "$IMG"
fi

debugfs -R "cat /etc/param/distributed_file.para" "$IMG" 2>/dev/null > "$PARA_TMP"
grep -q '^const.distributed_file_property.enabled=true$' "$PARA_TMP" || {
    echo "[lb-image] param already off (or missing):"
    grep const "$PARA_TMP" || true
    exit 0
}
sed -i 's/^const.distributed_file_property.enabled=true$/const.distributed_file_property.enabled=false/' "$PARA_TMP"
printf 'rm /etc/param/distributed_file.para\ncd /etc/param\nwrite %s distributed_file.para\n' "$PARA_TMP" \
    | debugfs -w "$IMG" >/dev/null 2>&1
debugfs -R "cat /etc/param/distributed_file.para" "$IMG" 2>/dev/null | grep 'const\.'
rm -f "$PARA_TMP"
