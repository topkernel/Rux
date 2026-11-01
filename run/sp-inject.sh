#!/bin/bash
# sp-inject.sh — build the private system.img carrying the pbench probe
# into the OH boot for the stockperf lane. Same recipe as oh-launcher's
# pbref-inject.sh, but the trigger is bootevent.bootanimation.finished
# (fires t=14.4s on the reference kernel, t~243s on Rux — launcher.ready
# never fires on Rux) and the job writes opts.txt gating the long-tail
# benches so the Rux run stays inside one boot window.
#
# Usage: sp-inject.sh <private-system.img>   (created if missing)
# Env:   PROBE  probe binary (default /tmp/pbench)
set -euo pipefail

IMG="${1:?usage: sp-inject.sh <private-system.img>}"
PROBE="${PROBE:-/tmp/pbench}"
SRC="$(cd "$(dirname "$0")/.." && pwd)/run/oh-img/system.img"

[ -f "$PROBE" ] || { echo "probe missing: $PROBE" >&2; exit 1; }

if [ ! -f "$IMG" ]; then
    echo "[inject] reflink copy $(basename "$SRC") -> $IMG"
    cp --reflink=auto "$SRC" "$IMG"
fi

CFG_NEW="$(mktemp /tmp/key_enable-sp.cfg.XXXXXX)"
cat > "$CFG_NEW" <<'EOF'
{
    "jobs" : [{
            "name" : "post-fs-data",
            "cmds" : [
                "write /proc/sys/fs/verity/require_signatures 1"
            ]
        }, {
            "name" : "param:bootevent.bootanimation.finished",
            "condition" : "bootevent.bootanimation.finished=true",
            "cmds" : [
                "mkdir /data/local 0755 root root",
                "mkdir /data/local/pbench 0755 root root",
                "write /data/local/pbench/opts.txt skip=p5c2,p5c3,p5d,p5e,p5f,p3d",
                "write /data/local/pbench/p7n.txt 300",
                "start key_enable"
            ]
        }
    ],
    "services" : [{
            "name" : "key_enable",
            "path" : ["/system/bin/key_enable", "/data/local/pbench", "/data/local/pbench/pbench-results.txt"],
            "importance" : -20,
            "uid" : "root",
            "gid" : ["root"],
            "secon" : "u:r:key_enable:s0",
            "start-mode": "condition",
            "once": 1,
            "sandbox": 0,
            "console": 1
        }
    ]
}
EOF

echo "[inject] swapping cfg + binary in $IMG"
debugfs -w -R "rm /system/etc/init/key_enable.cfg" "$IMG" >/dev/null 2>&1 || true
debugfs -w -R "write $CFG_NEW /system/etc/init/key_enable.cfg" "$IMG" >/dev/null
debugfs -w -R "sif /system/etc/init/key_enable.cfg mode=0100644" "$IMG" >/dev/null 2>&1 || true
debugfs -w -R "sif /system/etc/init/key_enable.cfg uid=0 gid=0" "$IMG" >/dev/null 2>&1 || true
debugfs -w -R "rm /system/bin/key_enable" "$IMG" >/dev/null 2>&1 || true
debugfs -w -R "write $PROBE /system/bin/key_enable" "$IMG" >/dev/null
debugfs -w -R "sif /system/bin/key_enable mode=0100755" "$IMG" >/dev/null 2>&1 || true
debugfs -w -R "sif /system/bin/key_enable uid=0 gid=0" "$IMG" >/dev/null 2>&1 || true

VRF="$(mktemp /tmp/pbench-verify-sp.XXXXXX)"
debugfs -R "dump /system/bin/key_enable $VRF" "$IMG" >/dev/null 2>&1
cmp "$PROBE" "$VRF" && echo "[inject] binary round-trip OK" || { echo "[inject] BINARY MISMATCH" >&2; exit 1; }
rm -f "$VRF" "$CFG_NEW"
echo "[inject] done: $IMG (pbench on bootevent.bootanimation.finished)"
