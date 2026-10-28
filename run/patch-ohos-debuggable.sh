#!/bin/bash
# patch-ohos-debuggable.sh — leg-40C candidate mitigation for the
# launcher-main writev wedge, applied to run/oh-img-p/system.img
# /system/etc/param/ohos.para (inode 1467).
#
# Evidence so far (sc349/sc350, tip5 census): after the
# AtomicServiceAbilityManager markers the launcher main stops making
# logical progress for the rest of its (now 20-min) life — never
# parks in any census-able state, no further ArkTS markers — and the
# single census catch shows it RUNNING in writev+0x13 (fd=3, iovec on
# its own stack, 3 iovecs = the hilogd socket write path). hilogd
# itself polls normally. Working theory: the debug-level log flood
# (the image ships const.debuggable=1) backs up / spins the app in
# hilog writev under Rux's slower syscall+socket throughput.
#
# Patch: const.debuggable=1 -> 0 (same byte length, file offset 173).
# This turns off app debug-mode logging at the param source
# (IsDeveloperModeOpen also flips false). Reversible.
#
# NOT APPLIED by default — apply only after the wedge is confirmed
# as a writev loop/hang by repeated census samples (sc352/sc353 tip6).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/run/oh-img-p/system.img"
INODE=1467
FOFF=173
SAVE="$ROOT/run/forensics-leg40B/ohos.para.orig"

mkdir -p "$ROOT/run/forensics-leg40B"
[ -f "$SAVE" ] || debugfs -R "dump <${INODE}> $SAVE" "$IMG" >/dev/null 2>&1

python3 - "$IMG" "$INODE" "$FOFF" "$SAVE" <<'EOF'
import subprocess, sys
img, inode, foff, save = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
orig = open(save, "rb").read()
line = b"const.debuggable="
i = orig.find(line)
assert i >= 0 and orig[i+len(line)] == ord("1"), "unexpected original"
assert i + len(line) == foff, f"value at {i+len(line)} expected {foff}"
BS = 4096
blk = foff // BS
out = subprocess.run(['debugfs', '-R', f'bmap <{inode}> {blk}', '--', img],
                     capture_output=True, text=True).stdout
pb = None
for l in out.splitlines():
    if l.strip().isdigit():
        pb = int(l); break
assert pb is not None, out
f = open(img, "rb"); f.seek(pb * BS + foff % BS); cur = f.read(1); f.close()
if cur == b"0":
    print("already patched"); sys.exit(0)
assert cur == b"1", cur
fd = open(img, "r+b"); fd.seek(pb * BS + foff % BS); fd.write(b"0"); fd.close()
print(f"patched: ohos.para fileoff {foff} '1'->'0' (const.debuggable off)")
EOF

debugfs -R "dump <${INODE}> $ROOT/run/forensics-leg40B/ohos-verify.para" "$IMG" >/dev/null 2>&1
grep -q "^const.debuggable=0$" "$ROOT/run/forensics-leg40B/ohos-verify.para" && echo "VERIFY OK"
