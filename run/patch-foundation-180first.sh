#!/bin/bash
# patch-foundation-180first.sh — leg-29B BMS-wall ordering fix, applied to
# run/oh-img-p/system.img /system/profile/foundation.json (inode 5307).
#
# Bug (leg-29B): on quiet draws the launcher never starts (zero
# StartHighestPriorityAbility, zero spawns, screens black after
# bootevent.bootanimation.finished). Chain: accountmgr's
# ActivateDefaultOsAccount -> SendToAMSAccountStart -> AMS
# StartUser(userId 100) -> UserController::SetForegroundUserId ->
# SwitchToUser -> StartHighestPriorityAbility (launcher). AMS = SA 180
# inside foundation (profile foundation.json, 8th entry); foundation's
# SA loader publishes 180 only after SA 401 (BMS)'s 14-34 s first-scan
# OnStart task, so Publish SA:180 lands within seconds of accountmgr's
# activation attempt — a coin-flip race (sc309/306/307 lost: publish
# AFTER activation, launcher never started; sc310 won: publish 4 lines
# before; sc308 recovered via a late UnlockUser path).
#
# Fix: move the SA 180 entry to the HEAD of the "systemability" list
# (pure byte-for-byte reorder, same file size, in-place write via the
# debugfs bmap mechanics — precedent run/patch-ldmusl-alignguard.sh).
# SA 180 then loads/publishes first (its own publish spend is ~25-70 ms)
# instead of behind SA 401's long OnStart, removing the race window.
#
# Run from repo root:  bash run/patch-foundation-180first.sh
# Idempotent: re-running finds 180 already first and exits 0.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/run/oh-img-p/system.img"
SRC="$ROOT/run/forensics-leg29B/foundation.json"
DST="$ROOT/run/forensics-leg29B/foundation-180first.json"
INODE=5307

python3 - "$SRC" "$DST" <<'EOF'
import json, sys
src, dst = sys.argv[1], sys.argv[2]
raw = open(src).read()
d = json.loads(raw)
if d['systemability'][0]['name'] == 180:
    print("already 180-first; nothing to do"); sys.exit(0)
i = raw.find('"name": 180')
start = raw.rfind('{', 0, i)
depth = 0; end = None
for k in range(start, len(raw)):
    if raw[k] == '{': depth += 1
    elif raw[k] == '}':
        depth -= 1
        if depth == 0:
            end = k + 1; break
entry = raw[start:end]
assert raw[end:end+2] == ',\n', repr(raw[end:end+2])
new = raw[:start] + raw[end+2:]
m = new.find('"systemability": [')
insert_at = new.find('\n', m) + 1
new = new[:insert_at] + entry + ',\n' + new[insert_at:]
assert len(new) == len(raw), (len(new), len(raw))
assert json.loads(new)['systemability'][0]['name'] == 180
open(dst, 'w').write(new)
print("built", dst, "same size", len(new))
EOF

python3 - "$IMG" "$SRC" "$DST" "$INODE" <<'EOF'
import subprocess, sys
img, src, dst, inode = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
s = open(src, 'rb').read(); d = open(dst, 'rb').read()
if s == d:
    print("image already patched"); sys.exit(0)
assert len(s) == len(d)
diffs = []; i = 0
while i < len(s):
    if s[i] != d[i]:
        j = i
        while j < len(s) and s[j] != d[j]: j += 1
        diffs.append((i, j)); i = j
    else: i += 1
BS = 4096
def bmap(off):
    blk = off // BS
    out = subprocess.run(['debugfs', '-R', f'bmap <{inode}> {blk}', '--', img],
                         capture_output=True, text=True).stdout
    for line in out.splitlines():
        if line.strip().isdigit(): return int(line)
    raise SystemExit(f"bmap failed for block {blk}: {out!r}")
fd = open(img, 'r+b')
for a, b in diffs:
    off = a; remaining = b - a
    while remaining > 0:
        blk = bmap(off); within = off % BS; n = min(BS - within, remaining)
        fd.seek(blk * BS + within); fd.write(d[off:off+n])
        off += n; remaining -= n
fd.close(); print("patched ok")
EOF

debugfs -R "dump <${INODE}> $ROOT/run/forensics-leg29B/foundation-verify.json" "$IMG" >/dev/null 2>&1
cmp "$ROOT/run/forensics-leg29B/foundation-verify.json" "$DST" && echo "VERIFY OK: image foundation.json == 180-first"
