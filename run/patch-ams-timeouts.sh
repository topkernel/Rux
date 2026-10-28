#!/bin/bash
# patch-ams-timeouts.sh — leg-40C AMS-lifecycle-timeout image-side
# mitigation (effective variant), applied to run/oh-img-p/system.img
# /system/etc/ams_service_config.json.
#
# Context: sc347/sc348 (after the hicollie detach fix) show launcher
# instances running their full cold start — including the launcher's
# own ArkTS code — then dying exit(0) at AMS's ability-load lifecycle
# timeout. sc348 measured (txn-timer anchored): HALF_TIMEOUT_WARNING
# at ~53 s, full TIMEOUT at ~86 s after spawn. This image's
# libabilityms.z.so does NOT read the
# persist.sys.abilityms.timeout_unit_time_ratio param (0 string hits;
# that knob is newer master code), so the appfwk.para ratio patch was
# ineffective — the effective knobs are the JSON keys in
# /system/etc/ams_service_config.json:
#   "app_not_response_process_timeout_time" : 1000
#   "ams_timeout_time" : 180
#   "timeout_unit_time" : 1000
# (loaded by AmsConfigurationParameter::LoadAppConfigurationForStartUpService).
#
# Patch (all same byte length, no file-size change):
#   ams_timeout_time:                      180 -> 999
#   app_not_response_process_timeout_time: 1000 -> 9999
#   timeout_unit_time:                     1000 -> 9999
# Empirically the full timeout was ~86 s with the old values; the
# worst-case single-key scaling (5.5x on ams) gives ~8 min; more if
# the formula combines the keys. Bring-up budget, not a product fix.
#
# REVERSIBLE: original saved in run/forensics-leg40B/.
# Run from repo root:  bash run/patch-ams-timeouts.sh
# Idempotent: exits 0 if the image already carries the patch.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/run/oh-img-p/system.img"
SAVE="$ROOT/run/forensics-leg40B/ams_service_config.json.orig"

mkdir -p "$ROOT/run/forensics-leg40B"
[ -f "$SAVE" ] || debugfs -R "dump /etc/ams_service_config.json $SAVE" "$IMG" >/dev/null 2>&1

python3 - "$IMG" "$SAVE" <<'EOF'
import json, subprocess, sys, re
img, save = sys.argv[1], sys.argv[2]
orig = open(save).read()
d = json.loads(orig)
assert d["service_startup_config"]["ams_timeout_time"] == 180, d
assert d["service_startup_config"]["app_not_response_process_timeout_time"] == 1000, d
assert d["service_startup_config"]["timeout_unit_time"] == 1000, d
# Same-length textual edits on the RAW text (never re-serialize JSON —
# size/format must stay byte-identical apart from the digits).
edits = [
    ('"ams_timeout_time" : 180', '"ams_timeout_time" : 999'),
    ('"app_not_response_process_timeout_time" : 1000', '"app_not_response_process_timeout_time" : 9999'),
    ('"timeout_unit_time" : 1000', '"timeout_unit_time" : 9999'),
]
text = orig
for a, b in edits:
    assert len(a) == len(b), (a, b)
    assert text.count(a) == 1, a
    text = text.replace(a, b)
assert len(text) == len(orig)
# locate inode + block of the three digit spans via their offsets
diffs = [i for i in range(len(orig)) if orig[i] != text[i]]
# group into the three value spans
spans = []
start = diffs[0]
prev = diffs[0]
for i in diffs[1:]:
    if i != prev + 1:
        spans.append((start, prev))
        start = i
    prev = i
spans.append((start, prev))
assert len(spans) == 3, spans
inode_out = subprocess.run(['debugfs', '-R', 'stat /etc/ams_service_config.json', '--', img],
                           capture_output=True, text=True).stdout
inode = int(re.search(r'Inode:\s+(\d+)', inode_out).group(1))
fd = open(img, 'r+b')
BS = 4096
written = []
for s, e in spans:
    blk = s // BS
    out = subprocess.run(['debugfs', '-R', f'bmap <{inode}> {blk}', '--', img],
                         capture_output=True, text=True).stdout
    pb = None
    for line in out.splitlines():
        if line.strip().isdigit():
            pb = int(line); break
    assert pb is not None, out
    off = pb * BS + s % BS
    fd.seek(off)
    fd.write(text[s:e+1].encode())
    written.append(hex(s))
fd.close()
print(f"patched ams_service_config.json (inode {inode}): spans {written} (999 / 9999 / 9999)")
EOF

debugfs -R "dump /etc/ams_service_config.json $ROOT/run/forensics-leg40B/ams_service_config.verify.json" "$IMG" >/dev/null 2>&1
python3 - "$ROOT/run/forensics-leg40B/ams_service_config.verify.json" <<'EOF'
import json, sys
d = json.load(open(sys.argv[1]))
s = d["service_startup_config"]
assert s["ams_timeout_time"] == 999 and s["app_not_response_process_timeout_time"] == 9999 \
    and s["timeout_unit_time"] == 9999, s
print("VERIFY OK: ams=999 anr=9999 unit=9999")
EOF
