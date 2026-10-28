#!/bin/bash
# patch-hicollie-skipjoin.sh — leg-40B launcher-join wall mitigation,
# applied to run/oh-img-p/system.img
# /system/lib64/chipset-sdk-sp/libhicollie.z.so (inode 2543;
# platformsdk is a symlink to it, chipset-pub-sdk has no copy).
#
# Wall (sc343/sc344, sleepsnap-PARK user-stack dumps + offline
# byte-match): after the setnowait param fix let the chain reach the
# launcher spawn, EVERY launcher instance parks its MAIN thread in
# pthread_join — decoded chain: libappkit_native -> libc++
# std::thread::join -> libhicollie Watchdog stop/init path (frames
# fileoff 0x12fa5 / 0x17c83; the joined thread's name "watchdog" is
# on the stack) -> LIFECYCLE_TIMEOUT kill by AppDfr -> respawn loop
# (31-56 instances/boot), no launcher pixels ever.
#
# The join site (disasm): the hicollie function at vaddr 0x13e90 sets
# the stop flag (xchg), does condition_variable::notify_all, then
#     13fa0: e8 9b 43 02 00  call std::__h::thread::join()@plt
# and the launcher main parks at that call's return (0x13fa5) — the
# watchdog thread never exits despite stop+notify.
#
# Patch: NOP the 5-byte call at vaddr 0x13fa0 (file offset 0x12fa0):
# e8 9b 43 02 00 -> 90 90 90 90 90. The join is skipped; execution
# proceeds to the following mov (join's return value is ignored in
# this path). The watchdog thread leaks (never joined) — a bring-up
# shortcut in the same spirit as the authorized DFX-facility skips:
# the launcher loses hicollie watchdog protection until un-patched.
# REVERSIBLE: original file saved in run/forensics-leg40B/.
#
# Run from repo root:  bash run/patch-hicollie-skipjoin.sh
# Idempotent: exits 0 if the image already carries the patch.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/run/oh-img-p/system.img"
INODE=2543
FOFF=0x12fa0
ORIG=0xe8
SAVE="$ROOT/run/forensics-leg40B/libhicollie.z.so.orig"

mkdir -p "$ROOT/run/forensics-leg40B"
[ -f "$SAVE" ] || debugfs -R "dump <${INODE}> $SAVE" "$IMG" >/dev/null 2>&1

python3 - "$IMG" "$INODE" "$FOFF" "$SAVE" <<'EOF'
import subprocess, sys
img, inode, foff, save = sys.argv[1], int(sys.argv[2]), int(sys.argv[3], 16), sys.argv[4]
orig = open(save, "rb").read()
assert orig[foff:foff+5] == bytes.fromhex("e89b430200"), orig[foff:foff+5].hex()
BS = 4096
blk = foff // BS
out = subprocess.run(['debugfs', '-R', f'bmap <{inode}> {blk}', '--', img],
                     capture_output=True, text=True).stdout
pb = None
for line in out.splitlines():
    if line.strip().isdigit():
        pb = int(line); break
if pb is None:
    raise SystemExit(f"bmap failed: {out!r}")
fd = open(img, "rb")
fd.seek(pb * BS + foff % BS)
cur = fd.read(5)
fd.close()
if cur == bytes.fromhex("9090909090"):
    print("image already patched; nothing to do")
    sys.exit(0)
assert cur == bytes.fromhex("e89b430200"), f"unexpected bytes {cur.hex()} at {foff:#x}"
fd = open(img, "r+b")
fd.seek(pb * BS + foff % BS)
fd.write(bytes([0x90] * 5))
fd.close()
print(f"patched: fileoff {foff:#x} call join@plt -> 5x nop (inode {inode})")
EOF

debugfs -R "dump <${INODE}> $ROOT/run/forensics-leg40B/libhicollie-verify.z.so" "$IMG" >/dev/null 2>&1
python3 - "$SAVE" "$ROOT/run/forensics-leg40B/libhicollie-verify.z.so" <<'EOF'
import sys
a = open(sys.argv[1], 'rb').read(); b = open(sys.argv[2], 'rb').read()
assert len(a) == len(b)
diffs = [i for i in range(len(a)) if a[i] != b[i]]
assert diffs == list(range(0x12fa0, 0x12fa5)), [hex(x) for x in diffs]
print("VERIFY OK: 5 bytes at 0x12fa0 NOPed (call join -> nop)")
EOF
