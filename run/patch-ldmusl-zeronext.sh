#!/bin/bash
# patch-ldmusl-zeronext.sh — write the waiter-next-zero patch into
# system.img's /system/lib/ld-musl-x86_64.so.1 (inode 2447) in place,
# via raw block writes (debugfs bmap), so the inode/metadata stay
# untouched. Idempotent: re-running rewrites the same bytes.
#
# Patch (see SC-SESSIONS leg-22): pthread_cond_timedwait's enqueue
# (vaddr 0x1182ea) never initializes the waiter node's next link
# (node = rbp-0x50, *(node+0) untouched by the enqueue; the walk in
# apply() follows next-links in nwaiters=-1 timeout mode and faults
# writing into whatever the stale stack slot points at — the launcher
# crash-loop). The patch routes the enqueue through two pad caves and
# adds `movq $0,(%rcx)` (zero the fresh node's next) before the
# tail/head linking.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/run/oh-img-p/system.img"
SRC="$ROOT/run/forensics-leg22/ld-musl-x86_64.so.1"
DST="$ROOT/run/forensics-leg22/ld-musl-x86_64-patched.so.1"
INODE=2447

[ -f "$DST" ] || { echo "missing $DST (build it first)"; exit 1; }

python3 - "$IMG" "$SRC" "$DST" "$INODE" <<'EOF'
import subprocess, sys, sysconfig
img, src, dst, inode = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
s = open(src,'rb').read(); d = open(dst,'rb').read()
assert len(s) == len(d)
diffs = []
i = 0
while i < len(s):
    if s[i] != d[i]:
        j = i
        while j < len(s) and s[j] != d[j]: j += 1
        diffs.append((i, j))
        i = j
    else:
        i += 1
print("regions:", [(hex(a), hex(b), b-a) for a,b in diffs])
BS = 4096
def bmap(off):
    blk = off // BS
    out = subprocess.run(['debugfs','-R',f'bmap <{inode}> {blk}','--',img],
                         capture_output=True, text=True).stdout
    for line in out.splitlines():
        line = line.strip()
        if line.isdigit():
            return int(line)
    raise SystemExit(f"bmap failed for block {blk}: {out!r}")
fd = open(img, 'r+b')
for a, b in diffs:
    off = a; remaining = b - a
    while remaining > 0:
        blk = bmap(off)
        within = off % BS
        n = min(BS - within, remaining)
        fd.seek(blk * BS + within)
        fd.write(d[off:off+n])
        print(f"wrote {n} bytes at file {off:#x} (fs block {blk}, +{within:#x})")
        off += n; remaining -= n
fd.close()
print("patched ok")
EOF
# verify: re-extract and compare
debugfs -R "dump <2447> $ROOT/run/forensics-leg22/libc-verify.so" "$IMG" >/dev/null 2>&1
cmp "$ROOT/run/forensics-leg22/libc-verify.so" "$DST" && echo "VERIFY OK: image copy == patched"
