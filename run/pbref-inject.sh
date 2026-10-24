#!/bin/bash
# pbref-inject.sh — build the private system.img that carries the pbench
# probe into the OH boot (perf-report.md 8.3(A) delivery, Linux-reference
# side; the same recipe transfers to the Rux side on run day).
#
# Slot: key_enable — chosen from /system/etc/init/*.cfg because it is
#   (a) a DEDICATED exec'd binary (/system/bin/key_enable, not sa_main,
#       which is shared by every SA service), (b) uid root (probe needs
#       /data writes + reboot(POWER_OFF)), (c) once:1 (no respawn loop),
#   (d) non-essential (it only enables fs-verity signature requirement;
#       dropping it does not change the boot's service shape otherwise).
# The stock start trigger (job "init": start key_enable, ~t=2.7s — too
# early) is replaced with the image's own late-start pattern
# (audio_server.cfg): a param job conditioned on
# bootevent.launcher.ready=true — t=10.1s on the reference kernel
# (oh-ref-r1), and exactly the run-day milestone on the Rux side.
#
# Edits (PRIVATE copy only; shared images are never touched):
#   /system/bin/key_enable            -> the pbench binary (0755 root:root)
#   /system/etc/init/key_enable.cfg   -> stock jobs kept, start moved to
#        the launcher.ready param job which mkdirs /data/local/pbench;
#        service path gains args (workdir, results file), sandbox=0,
#        console=1 (PBENCH lines land on /dev/console -> serial).
#
# Usage: pbref-inject.sh <private-system.img>   (created if missing)
# Env:   PROBE  probe binary (default $ROOT/work/pbench)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="${1:?usage: pbref-inject.sh <private-system.img>}"
PROBE="${PROBE:-$ROOT/work/pbench}"
SRC="$ROOT/run/oh-img/system.img"

[ -f "$PROBE" ] || { echo "probe missing: $PROBE (build: musl-gcc -static -O2 test/pbench.c)" >&2; exit 1; }

if [ ! -f "$IMG" ]; then
    echo "[inject] reflink copy $(basename "$SRC") -> $IMG"
    cp --reflink=auto "$SRC" "$IMG"
fi

CFG_NEW="$(mktemp /tmp/key_enable.cfg.XXXXXX)"
cat > "$CFG_NEW" <<'EOF'
{
    "jobs" : [{
            "name" : "post-fs-data",
            "cmds" : [
                "write /proc/sys/fs/verity/require_signatures 1",
                "mkdir /data/service/el1/public/profiles 0655 installs installs",
                "mkdir /data/service/el1/public/profiles/release 0655 installs installs",
                "mkdir /data/service/el1/public/profiles/debug 0655 installs installs"
            ]
        }, {
            "name" : "param:bootevent.launcher.ready",
            "condition" : "bootevent.launcher.ready=true",
            "cmds" : [
                "mkdir /data/local 0755 root root",
                "mkdir /data/local/pbench 0755 root root",
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

# verify round-trip: dumped bytes must match what we wrote
VRF="$(mktemp /tmp/pbench-verify.XXXXXX)"
debugfs -R "dump /system/bin/key_enable $VRF" "$IMG" >/dev/null 2>&1
cmp "$PROBE" "$VRF" && echo "[inject] binary round-trip OK" || { echo "[inject] BINARY MISMATCH" >&2; exit 1; }
rm -f "$VRF" "$CFG_NEW"

echo "[inject] done: $IMG (key_enable slot -> pbench, start on bootevent.launcher.ready)"
