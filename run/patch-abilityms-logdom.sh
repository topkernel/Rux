#!/bin/bash
# patch-abilityms-logdom.sh — leg-29B AMS visibility patch, applied to
# run/oh-img-p/system.img /system/lib64/platformsdk/libabilityms.z.so
# (inode 4520).
#
# Problem: the entire AMS hilog domain 0xd001336 never reaches the
# serial console on this image (only the radar sub-domain prints), so
# the launcher-start chain inside foundation is invisible: on walled
# quiet draws accountmgr's SendToAMSAccountStart returns 0 (its
# ERROR-level failure logs never fire), yet AMS's SwitchToUser never
# runs (zero StartResidentAbility / launcher-start lines). Where the
# chain dies between SendRequest(code 46) and SwitchToUser is not
# observable with the stock logs.
#
# Patch: retarget the HiLogPrint DOMAIN constant of exactly four log
# sites in libabilityms from 0xd001336 (silent) to 0xd001b00
# (AccountMgrService's domain — proven to print on this image):
#   vaddr 0x2c1df0  StartUser entry        "startUser in service:%d, displayId:%lu"
#   vaddr 0x34144a  SwitchToUser entry     "...oldUserId:%d, newUserId:%d, isAppRecovery:%d"
#   vaddr 0x341eac  StartHighestPriorityAbility generic entry
#   vaddr 0x3420d5  StartHighestPriorityAbility "waiting query highest priority ability info completed"
# Each site is the 5-byte sequence `ba 36 13 00 0d` (mov $0xd001336,
# %edx) replaced by `ba 00 1b 00 0d` (mov $0xd001b00, %edx) — same
# length, in-place write via the debugfs bmap mechanics (precedent
# run/patch-ldmusl-alignguard.sh). The whole-domain remap (4791 sites)
# was deliberately NOT done: AMS is chatty and console volume changes
# the boot phenotype (the wall/advance split is timing-sensitive).
#
# Run from repo root:  bash run/patch-abilityms-logdom.sh
# Idempotent: exits 0 if the image copy already matches.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/run/oh-img-p/system.img"
SRC="$ROOT/run/forensics-leg29B/libabilityms-verify.so"
INODE=4520

python3 - "$IMG" "$SRC" "$INODE" <<'EOF'
import subprocess, sys
img, ref, inode = sys.argv[1], sys.argv[2], int(sys.argv[3])
cur = subprocess.run(['debugfs','-R',f'dump <{inode}> /dev/stdout','--',img],
                     capture_output=True).stdout
want = open(ref,'rb').read()
if cur == want:
    print("image already patched"); sys.exit(0)
# the reference is the ORIGINAL dump + 4 site patches; recompute from cur
sites = {0x2c0df0:'StartUser entry',0x34044a:'SwitchToUser entry',
         0x340eac:'StartHPA generic',0x3410d5:'StartHPA waiting'}
old = bytes.fromhex('ba3613000d'); new = bytes.fromhex('ba001b000d')
d = bytearray(cur)
n = 0
for off, name in sites.items():
    if d[off:off+5] == new:
        continue
    assert d[off:off+5] == old, (name, hex(off), d[off:off+5].hex())
    d[off:off+5] = new; n += 1
BS = 4096
def bmap(o):
    blk = o // BS
    out = subprocess.run(['debugfs','-R',f'bmap <{inode}> {blk}','--',img],
                         capture_output=True, text=True).stdout
    for line in out.splitlines():
        if line.strip().isdigit(): return int(line)
    raise SystemExit(f'bmap failed for block {blk}: {out!r}')
fd = open(img, 'r+b')
for off in sites:
    blk = bmap(off); within = off % BS
    fd.seek(blk*BS + within); fd.write(d[off:off+5])
fd.close(); print(f"patched {n} site(s)")
EOF

debugfs -R "dump <${INODE}> $ROOT/run/forensics-leg29B/libabilityms-verify2.so" "$IMG" >/dev/null 2>&1
python3 - "$ROOT/run/forensics-leg29B/libabilityms-verify2.so" <<'EOF'
import sys
d = open(sys.argv[1],'rb').read()
new = bytes.fromhex('ba001b000d')
for off in (0x2c0df0, 0x34044a, 0x340eac, 0x3410d5):
    assert d[off:off+5] == new, hex(off)
print("VERIFY OK: 4 AMS log sites retargeted to 0xd001b00")
EOF
