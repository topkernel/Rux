#!/bin/bash
# patch-appfwk-timeoutratio.sh — leg-40C AMS-lifecycle-timeout image-side
# mitigation, applied to run/oh-img-p/system.img
# /system/etc/param/appfwk.para (inode 1392).
#
# Wall (sc347, after the hicollie detach patch removed the launcher
# main's eternal pthread_join): launcher instances now run their full
# cold start — appspawn sandbox, ArkTS runtime boot, launcher's own
# ArkTS module code (Launcher_Default: LauncherAbilityManager,
# CommonBundleManager, OsAccountJsKit napi async work) — but AMS kills
# each instance at the ~41 s ability-load lifecycle timeout
# (LIFECYCLE_HALF_TIMEOUT_WARNING + LIFECYCLE_TIMEOUT_WARNING,
# AMSI7903, then clean exit(0); measured spawn->exit ~= 41 s, serial
# txn-timer anchored). The reference Linux boot completes the same
# startup in seconds; under Rux the cold start needs longer than the
# window, so no instance ever reaches OnForeground.
#
# The timeout scales with a system parameter:
#   AmsConfigurationParameter::GetAppStartTimeoutTime()
#     = timeoutUnitTime_ (default 1000 ms, no JSON override in image)
#       * AppUtils::GetTimeoutUnitTimeRatio()
#     = persist.sys.abilityms.timeout_unit_time_ratio (default 1)
# (ability_ability_runtime services/abilitymgr/src/
#  ams_configuration_parameter.cpp:96-98; default shipped in
#  /system/etc/param/appfwk.para line 25; scgold userdata carries no
#  persisted override — /service/startup/parameter* dirs are empty).
#
# Patch: the value byte of that param line (file offset 1223):
# '1' -> '9' (same byte length, no size change). Ratio 9 scales the
# whole lifecycle-timeout family (load, foreground, ANR) by 9 ->
# ~6 min per launch attempt.
#
# REVERSIBLE: original file saved in run/forensics-leg40B/.
# Run from repo root:  bash run/patch-appfwk-timeoutratio.sh
# Idempotent: exits 0 if the image already carries the patch.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/run/oh-img-p/system.img"
INODE=1392
FOFF=1223
SAVE="$ROOT/run/forensics-leg40B/appfwk.para.orig"

mkdir -p "$ROOT/run/forensics-leg40B"
[ -f "$SAVE" ] || debugfs -R "dump <${INODE}> $SAVE" "$IMG" >/dev/null 2>&1

python3 - "$IMG" "$INODE" "$FOFF" "$SAVE" <<'EOF'
import subprocess, sys
img, inode, foff, save = sys.argv[1], int(sys.argv[2]), int(sys.argv[3], 16) if sys.argv[3].startswith("0x") else int(sys.argv[3]), sys.argv[4]
orig = open(save, "rb").read()
line = b"persist.sys.abilityms.timeout_unit_time_ratio = "
i = orig.find(line)
assert i >= 0, "param line not found in saved original"
voff = i + len(line)
assert voff == foff, f"line value at {voff}, expected {foff} — adjust FOFF"
assert orig[voff] == ord("1"), f"unexpected original value byte {orig[voff]!r}"
BS = 4096
blk = foff // BS
out = subprocess.run(['debugfs', '-R', f'bmap <{inode}> {blk}', '--', img],
                     capture_output=True, text=True).stdout
pb = None
for line_ in out.splitlines():
    if line_.strip().isdigit():
        pb = int(line_); break
if pb is None:
    raise SystemExit(f"bmap failed: {out!r}")
f = open(img, "rb")
f.seek(pb * BS + foff % BS)
cur = f.read(1)
f.close()
if cur == b"9":
    print("image already patched (ratio 9); nothing to do")
    sys.exit(0)
assert cur == b"1", f"unexpected live byte {cur!r} at fileoff {foff}"
fd = open(img, "r+b")
fd.seek(pb * BS + foff % BS)
fd.write(b"9")
fd.close()
print(f"patched: appfwk.para fileoff {foff} '1' -> '9' (timeout ratio x9, inode {inode})")
EOF

debugfs -R "dump <${INODE}> $ROOT/run/forensics-leg40B/appfwk-verify.para" "$IMG" >/dev/null 2>&1
grep -q "persist.sys.abilityms.timeout_unit_time_ratio = 9" "$ROOT/run/forensics-leg40B/appfwk-verify.para"
echo "VERIFY OK: ratio param now 9"
