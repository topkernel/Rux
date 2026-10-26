#!/bin/bash
# patch-ldmusl-alignguard.sh — leg-28 launcher-crash patch, applied on top
# of the leg-22-patched ld-musl inside run/oh-img-p/system.img (inode 2447).
#
# Bug (SC-SESSIONS leg-27): the private-condvar signal walk
# (`lock cmpxchg %edx,0x10(%rcx)` at vaddr 0x1187d2 in the helper that
# follows pthread_cond_timedwait) walks the waiter list under the cond
# lock and reaches a stale/unlinked waiter node whose memory was
# recycled into a read-only file mapping (the fork-dropped binder-buffer
# range); the walk's read of node->next succeeds (page is readable) and
# the CAS write faults with ACCERR — killing m.ohos.launcher (and the
# dataability processes) in a deterministic crash-loop. Every logged
# fault has the bad node at ...a92c (cr2 ...a93c), i.e. 12 mod 16, while
# every legitimate waiter node is rbp-0x50 of a 16-aligned frame.
#
# Patch (pad-cave, same mechanics as leg-22):
#   site 0x1187d0: `xor %eax,%eax; jmp GUARD`
#   GUARD 0x118143: `test $0xf,%cl; jne T; jmp CAS`
#   T 0x11814d: `jmp BAIL`
#   BAIL 0x1181a6: `xor %ecx,%ecx; jmp 0x118825`  (the walk's
#        list-empty split path: cond head/tail zeroed, lock released,
#        already-marked waiters still woken and acked, corrupt remainder
#        dropped instead of dereferenced)
#   CAS 0x118d94: replay `lock cmpxchg %edx,0x10(%rcx); jmp 0x1187d7`
# A 16-misaligned "node" can never be a real stack waiter; on such a
# node the walk now empties the (already corrupt) list and returns
# normally instead of faulting. Aligned nodes take the original path
# byte-for-byte.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/run/oh-img-p/system.img"
BASE="$ROOT/run/forensics-leg22/ld-musl-x86_64-patched.so.1"
DST="$ROOT/run/forensics-leg27/ld-musl-x86_64-alignguard.so.1"
INODE=2447

python3 - "$BASE" "$DST" <<'EOF'
import sys
src, dst = sys.argv[1], sys.argv[2]
d = bytearray(open(src,'rb').read())
assert len(d) == 1295280, len(d)

regions = [
    # (file_off, bytes, what)
    (0x1177d0, bytes.fromhex('31c0e96cf9ffff'), 'site: xor eax; jmp GUARD'),
    (0x117143, bytes.fromhex('f6c10f7505e9470c0000'), 'GUARD: test $0xf,cl; jne T; jmp CAS'),
    (0x11714d, bytes.fromhex('eb57'), 'T: jmp BAIL'),
    (0x1171a6, bytes.fromhex('31c9e978060000'), 'BAIL: xor ecx; jmp 0x118825'),
    (0x117d94, bytes.fromhex('f00fb15110e939faffff'), 'CAS: replay cmpxchg; jmp 0x1187d7'),
]
# preconditions: original code at the site, cc padding in the caves
assert d[0x1177d0:0x1177d7] == bytes.fromhex('31c0f00fb15110'), d[0x1177d0:0x1177d7].hex()
for off, n in [(0x117143,13),(0x1171a6,10),(0x117d94,12)]:
    assert all(b == 0xcc for b in d[off:off+n]), (hex(off), d[off:off+n].hex())
for off, data, what in regions:
    d[off:off+len(data)] = data
    print(f"wrote {len(data):2d} bytes @ {off:#x}: {what}")
open(dst,'wb').write(bytes(d))
print("built", dst)
EOF

python3 - "$IMG" "$BASE" "$DST" "$INODE" <<'EOF'
import subprocess, sys
img, src, dst, inode = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
s = open(src,'rb').read(); d = open(dst,'rb').read()
assert len(s) == len(d)
diffs = []; i = 0
while i < len(s):
    if s[i] != d[i]:
        j = i
        while j < len(s) and s[j] != d[j]: j += 1
        diffs.append((i, j)); i = j
    else: i += 1
print("regions:", [(hex(a), b-a) for a, b in diffs])
BS = 4096
def bmap(off):
    blk = off // BS
    out = subprocess.run(['debugfs','-R',f'bmap <{inode}> {blk}','--',img],
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
        print(f"wrote {n} bytes at file {off:#x} (fs block {blk}, +{within:#x})")
        off += n; remaining -= n
fd.close(); print("patched ok")
EOF

# verify: re-extract and compare, then disassemble the patched walk
debugfs -R "dump <2447> $ROOT/run/forensics-leg27/libc-verify-ag.so" "$IMG" >/dev/null 2>&1
cmp "$ROOT/run/forensics-leg27/libc-verify-ag.so" "$DST" && echo "VERIFY OK: image copy == alignguard-patched"
objdump -d --start-address=0x118140 --stop-address=0x118160 "$DST" | tail -8
objdump -d --start-address=0x1187c0 --stop-address=0x1187e0 "$DST" | tail -10
