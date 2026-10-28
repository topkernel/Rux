#!/bin/bash
# patch-hicollie-detachjoin.sh — leg-40C launcher-wall image-side
# mitigation v2, applied to run/oh-img-p/system.img
# /system/lib64/chipset-sdk-sp/libhicollie.z.so (inode 2543;
# platformsdk is a symlink to it, chipset-pub-sdk has no copy).
#
# Wall (sc343/sc344 sleepsnap park dumps + offline byte-match): every
# launcher instance's MAIN thread parks forever in
#   appkit Watchdog::Stop() -> hicollie WatchdogInner::Stop()
#     -> notify_all -> std::thread::join()   (call at vaddr 0x13fa0)
#     -> musl pthread_join(&td->tid, FUTEX_WAIT|PRIVATE, NO timeout)
# The joined OS_DfxWatchdog worker is absent from every census in app
# children (services' workers are alive and healthy), so the exit-time
# clear+wake never arrives; the parked main is later SIGKILLed by
# appspawn's kickdog and AMS respawns — 40 instances/boot, no pixels.
#
# v1 (patch-hicollie-skipjoin.sh) NOPed the call. FLAW found by
# disassembly of the post-join sequence (0x13fa5): Stop() then does
# threadLoop_ = nullptr + ~thread() + operator delete — destroying a
# STILL-JOINABLE std::thread -> libc++ calls std::terminate() ->
# SIGABRT. v1 converts the eternal park into an instant process
# abort, not into a working Stop().
#
# v2 (this): redirect the call join@plt -> detach@plt (0x38340 ->
# 0x38730; 5 bytes e8 9b 43 02 00 -> e8 8b 47 02 00). detach() takes
# the same this-in-rdi (loaded at 0x13f8e), so the rel32 swap is the
# only change needed. Semantics: pthread_detach on the (dead or
# alive) worker; libc++ clears the id so joinable() is false and the
# destructor is legal; an alive worker still sees isNeedStop_ (set
# just before) and exits on its own; a dead worker's kernel cleartid
# frees its stack. Stop() then completes normally.
#
# REVERSIBLE: original file saved in run/forensics-leg40B/
# (libhicollie.z.so.orig). Run from repo root:
#   bash run/patch-hicollie-detachjoin.sh
# Idempotent: exits 0 if the image already carries the v2 patch.
# Handles both v0 (original call) and v1 (NOP) as input states.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/run/oh-img-p/system.img"
INODE=2543
FOFF=0x12fa0
SAVE="$ROOT/run/forensics-leg40B/libhicollie.z.so.orig"

mkdir -p "$ROOT/run/forensics-leg40B"
[ -f "$SAVE" ] || debugfs -R "dump <${INODE}> $SAVE" "$IMG" >/dev/null 2>&1

python3 - "$IMG" "$INODE" "$FOFF" "$SAVE" <<'EOF'
import subprocess, sys
img, inode, foff, save = sys.argv[1], int(sys.argv[2]), int(sys.argv[3], 16), sys.argv[4]
orig = open(save, "rb").read()
V0 = bytes.fromhex("e89b430200")   # call join@plt
V1 = bytes.fromhex("9090909090")   # v1: 5x nop
V2 = bytes.fromhex("e88b470200")   # v2: call detach@plt
assert orig[foff:foff+5] == V0, f"orig bytes {orig[foff:foff+5].hex()} != {V0.hex()}"
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
f = open(img, "rb")
f.seek(pb * BS + foff % BS)
cur = f.read(5)
f.close()
if cur == V2:
    print("image already carries v2 (detach redirect); nothing to do")
    sys.exit(0)
assert cur in (V0, V1), f"unexpected live bytes {cur.hex()} at fileoff {foff:#x}"
fd = open(img, "r+b")
fd.seek(pb * BS + foff % BS)
fd.write(V2)
fd.close()
src = "v0 join-call" if cur == V0 else "v1 nop"
print(f"patched: fileoff {foff:#x} {src} -> {V2.hex()} (call detach@plt, inode {inode})")
EOF

debugfs -R "dump <${INODE}> $ROOT/run/forensics-leg40B/libhicollie-detach-verify.z.so" "$IMG" >/dev/null 2>&1
python3 - "$SAVE" "$ROOT/run/forensics-leg40B/libhicollie-detach-verify.z.so" <<'EOF'
import sys
a = open(sys.argv[1], 'rb').read(); b = open(sys.argv[2], 'rb').read()
assert len(a) == len(b), (len(a), len(b))
diffs = [i for i in range(len(a)) if a[i] != b[i]]
# join-call e8 9b 43 02 00 vs detach-call e8 8b 47 02 00: only the
# rel32 bytes 1-2 differ; e8, 02, 00 are identical.
assert diffs == [0x12fa1, 0x12fa2], [hex(x) for x in diffs]
assert b[0x12fa0:0x12fa5] == bytes.fromhex("e88b470200")
assert a[0x12fa0:0x12fa5] == bytes.fromhex("e89b430200")
print("VERIFY OK: 0x12fa0 now e8 8b 47 02 00 (call detach@plt; was join@plt)")
EOF
