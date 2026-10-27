#!/bin/bash
# patch-ldmusl-guardread.sh — leg-31B render_service crash mitigation,
# applied on top of the current (leg-28 alignguard) ld-musl inside
# run/oh-img-p/system.img (inode 2447).
#
# Bug (SC-SESSIONS leg-31B, sc321B/sc324B/sc325B adjudication): OH's
# libdfx_signalhandler signal handler (watchdog stack sampling, signal
# 42) runs on the interrupted thread's own stack and copies a 936-byte
# (0x3a8) context block from a frame address near the stack top
# (memcpy helper at libdfx VMA 0xc839; call site VMA 0x93a4 loads
# ecx=0x3a8). OH's thread-stack allocator (this ld-musl, function near
# log_print: mmap len 0x9000 RW = [2-page guard][7-page stack], then
# mprotect(base, guard_global=0x2000, PROT_NONE) at VMA 0x118acb)
# leaves the guard PROT_NONE. When the sample lands on a just-created
# thread (sp == top - 0x478, right below struct pthread), the copy
# crosses the stack top into the guard -> SIGSEGV inside the handler
# -> render_service dies -> init stop-all sweep kills the foundation
# StartUser handler thread (the Gate-1 lottery) or accountmgr
# pre-activation. Kernel side adjudicated CLEAN (VMA origin stamp:
# guard origin=mprotect = the image's own syscall; stack origin=mmap;
# no +FORK).
#
# Patch (2 bytes, guard-protect): at file 0x117ac9 (VMA 0x118ac9)
#   `xor %edx,%edx`    (31 d2)  prot = PROT_NONE
# becomes
#   `mov $0x1,%dl`     (b2 01)  prot = PROT_READ
# edx provably holds 3 (PROT_READ|PROT_WRITE, the mmap prot argument,
# preserved by musl's mmap wrapper r13d save/restore) at this point,
# and its upper bytes are zero, so setting dl=1 yields edx=1 exactly.
# Effect: the guard stays UNWRITABLE (stack-overflow writes still
# fault — the guard's protective purpose is intact) but READABLE, so
# the dfx context read across the stack top returns bytes instead of
# killing the process. Idempotent; reversible by restoring 31 d2.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/run/oh-img-p/system.img"
DST="$ROOT/run/forensics-leg31B/ld-musl-x86_64-guardread.so.1"
INODE=2447
SITE=0x117ac9

mkdir -p "$ROOT/run/forensics-leg31B"
debugfs -R "dump <$INODE> $DST" "$IMG" >/dev/null 2>&1

python3 - "$DST" "$SITE" <<'EOF'
import sys
dst, site = sys.argv[1], int(sys.argv[2], 16)
d = bytearray(open(dst, 'rb').read())
assert len(d) == 1295280, len(d)
cur = bytes(d[site:site+7])
if cur[:2] == bytes.fromhex('b201'):
    print("already patched (b2 01) — idempotent no-op")
    sys.exit(0)
assert cur == bytes.fromhex('31d2e880d8fbff'), f"unexpected bytes: {cur.hex()}"
d[site:site+2] = bytes.fromhex('b201')          # mov $1,%dl
open(dst, 'wb').write(d)
print("patched: xor edx,edx -> mov $1,%dl (guard PROT_NONE -> PROT_READ)")
EOF

# write back into the image at the raw block (same mechanics as the
# alignguard script): bmap the file offset, write through.
python3 - "$IMG" "$INODE" "$SITE" "$DST" <<'EOF'
import sys, subprocess
img, inode, site, dst = sys.argv[1], sys.argv[2], int(sys.argv[3], 16), sys.argv[4]
BS = 4096
blk = int(subprocess.run(['debugfs','-R',f'bmap <{inode}> {site//BS}','--',img],
                         capture_output=True, text=True).stdout.strip().splitlines()[-1])
new = open(dst,'rb').read()
fd = open(img, 'r+b')
fd.seek(blk*BS + site % BS)
old = fd.read(2)
if old == new[site:site+2]:
    print("image already carries the patch")
else:
    assert old == bytes.fromhex('31d2'), old.hex()
    fd.seek(blk*BS + site % BS)
    fd.write(new[site:site+2])
    print(f"wrote 2 bytes at ld-musl file {site:#x} (fs block {blk})")
fd.close()
EOF

# verify: re-extract and disassemble the patched site
debugfs -R "dump <$INODE> $ROOT/run/forensics-leg31B/libc-verify-gr.so" "$IMG" >/dev/null 2>&1
cmp -s "$ROOT/run/forensics-leg31B/libc-verify-gr.so" "$DST" && echo "VERIFY OK: image copy == guardread-patched"
objdump -d --start-address=0x118ab0 --stop-address=0x118ad5 "$DST" | tail -9
