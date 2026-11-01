#!/bin/bash
# lb-mkimage3.sh — build run/system-lb3.img (private; never the central
# store): system-lb.img + the poison-mark-patched libipc_single.z.so.
#
# Why (lb61/lb62, ninth successor): OH's IPCObjectProxy::SendRequestInner
# permanently death-marks a proxy when a sync send returns ERR_DEAD_OBJECT
# (32). A server stub that returns an error with an empty reply parcel
# sends TF_STATUS_CODE+32, so a TRANSIENT server-side error (accountmgr
# mid-init answering GET_FOREGROUND_OS_ACCOUNTS with 32 during the
# 100-140s window on our boots) poisons the caller's proxy for the WHOLE
# boot. lb62's POISON32-DELIVER map: accountmgr -> foundation tid995/1043
# at 140.187; lb61: the same shape at ~101.9 -> CheckUserParam's
# IsExistOsAccount fails silently at 135.986 -> StartUser(100) returns
# 2097193 -> no SwitchToUser -> no launcher, forever.
#
# The patch removes ONLY the status-32 mark leg in SendRequestInner (the
# real death path, SendObituary on kernel death notice, still marks).
# Built from the oh-robot tree (whose out-tree libipc_single.z.so is
# bit-identical to the image copy, md5 fb4a215b...):
#   edit ipc_object_proxy.cpp (comment out the mark block)
#   ninja -w dupbuild=warn ipc_single   (prebuilt python on PATH)
#   save out/.../communication/ipc/libipc_single.z.so
#   REVERT the source edit and restore the out-tree .so.
# Patched copy kept at /tmp is rebuilt on demand; this script only
# re-injects a given .so into a fresh image copy.
set -euo pipefail
cd "$(dirname "$0")/.."

SRC="run/system-lb.img"
IMG="run/system-lb3.img"
SO="${1:-/tmp/ipcpatch/libipc_single.patched.z.so}"

if [ ! -f "$SO" ]; then
    echo "[lb3-image] patched .so not found at $SO — rebuild it first (see header)" >&2
    exit 1
fi

cp --reflink=auto "$SRC" "$IMG"
printf 'rm /system/lib64/platformsdk/libipc_single.z.so\ncd /system/lib64/platformsdk\nwrite %s libipc_single.z.so\n' "$SO" \
    | debugfs -w "$IMG" >/dev/null 2>&1
TMP=$(mktemp)
debugfs -R "dump /system/lib64/platformsdk/libipc_single.z.so $TMP" "$IMG" >/dev/null 2>&1
echo "[lb3-image] injected $(md5sum "$TMP" | cut -d' ' -f1) (source .so: $(md5sum "$SO" | cut -d' ' -f1))"
rm -f "$TMP"
