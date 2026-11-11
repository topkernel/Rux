#!/usr/bin/env python3
# hp94-parse.py v3 (94th): FULL-window walk of the hiperf PERFILE2
# record. The 93rd's v2 heuristic (find first type-9 run) landed mid-
# file and reached only ~2.5s; the file is actually ONE clean record
# stream from the header's data section (empirically 0x130 here):
# 244k samples walk with zero resyncs. This parser:
#   - walks every (u32 type, u16 misc, u16 size) record
#   - builds pid->comm + pid->[mmap module] tables from the meta records
#   - classifies kernel/user by IP RANGE (the record predates the 93rd's
#     uapi misc fix, so misc bits are the swapped encoding — trust IPs)
#   - emits: time-structure table, per-process kernel/user split, top
#     user modules, top user (module+offset) sites for the tail burners
# Usage: hp94-parse.py <record.data> [--syms dir]
import struct, sys, collections, subprocess, os

path = sys.argv[1]
symsdir = sys.argv[sys.argv.index('--syms') + 1] if '--syms' in sys.argv else '/tmp/ohlib'
data = open(path, 'rb').read()
n = len(data)

# header: PERFILE2 + u64s; the data section is the (off,size) pair with
# the LARGEST size (the 93rd's hp92a: off=0x130 size=0x2020a5d).
fields = struct.unpack_from('<6Q', data, 8)
best = None
for i in range(1, 5):
    off, size = fields[i], fields[i + 1]
    if 0x80 <= off < n and 0 < size <= n - off:
        if best is None or size > best[1]:
            best = (off, size)
data_off, data_size = best if best else (0x130, n - 0x130)
end = min(data_off + data_size, n)

TYPE_SAMPLE = 9
comm = {}          # pid -> comm (from type-3/4 records)
mmaps = collections.defaultdict(list)  # pid -> [(base, len, pgoff, name)]
samples = []       # (tstamp, pid, tid, cpu, ip, nr, frames_off)
tmin, tmax = None, None
pos = data_off
ntype = collections.Counter()
while pos + 8 <= end:
    rtype, misc, size = struct.unpack_from('<IHH', data, pos)
    if size < 8 or pos + size > end or rtype > 20:
        break
    ntype[rtype] += 1
    body = data[pos + 8: pos + size]
    if rtype == TYPE_SAMPLE and size >= 72:
        ident, ip = struct.unpack_from('<QQ', body, 0)
        pid, tid = struct.unpack_from('<II', body, 16)
        tstamp, sid, stream = struct.unpack_from('<QQQ', body, 24)
        cpu, _r = struct.unpack_from('<II', body, 48)
        nr = struct.unpack_from('<Q', body, 64)[0]
        samples.append((tstamp, pid, tid, cpu, ip, nr, pos + 8 + 72))
        if tmin is None or tstamp < tmin: tmin = tstamp
        if tmax is None or tstamp > tmax: tmax = tstamp
    elif rtype in (3, 4) and len(body) >= 24:  # comm-ish: pid,tid + name
        pid, tid = struct.unpack_from('<II', body, 0)
        name = body[8:].split(b'\0')[0].decode('utf-8', 'replace')
        if name:
            comm[pid] = name
    elif rtype == 10 and len(body) >= 72:      # mmap: pid,tid,addr,len,pgoff,...,name@64
        pid, tid = struct.unpack_from('<II', body, 0)
        base, ln, pgoff = struct.unpack_from('<QQQ', body, 8)
        name = body[64:].split(b'\0')[0].decode('utf-8', 'replace')
        if name:
            mmaps[pid].append((base, ln, pgoff, name))
    pos += size

print(f"# {path}: records={sum(ntype.values())} types={dict(ntype)}")
print(f"# samples={len(samples)} span={((tmax-tmin)/1e9 if tmax else 0):.1f}s"
      f" ({(tmin or 0)/1e9:.1f}-{(tmax or 0)/1e9:.1f}s)")
if not samples:
    sys.exit(0)

def is_kernel(ip):
    return (ip >> 44) == 0xFFFFFFF or (ip >> 32) == 0xFFFFFFFF

def mod_of(pid, ip):
    for base, ln, pgoff, name in mmaps.get(pid, ()):
        if base <= ip < base + ln:
            return name, ip - base + pgoff
    for p, lst in mmaps.items():
        # shared libraries: same mapping in a sibling proc (base may match)
        for base, ln, pgoff, name in lst:
            if base <= ip < base + ln:
                return name, ip - base + pgoff
    return None, 0

# ---- time structure: 5s buckets, kernel/user split ------------------
BUCKET = 5_000_000_000
b_kern = collections.Counter(); b_user = collections.Counter()
proc_kern = collections.Counter(); proc_user = collections.Counter()
user_mod = collections.Counter()
user_site = collections.Counter()   # (pid, module, off) — off rounded
for t, pid, tid, cpu, ip, nr, fo in samples:
    b = int((t - tmin) // BUCKET)
    if is_kernel(ip):
        b_kern[b] += 1; proc_kern[pid] += 1
    else:
        b_user[b] += 1; proc_user[pid] += 1
        m, off = mod_of(pid, ip)
        if m:
            user_mod[m] += 1
            user_site[(pid, m, off & ~0xF)] += 1

print("\n== time buckets (5s, rel): kern / user ==")
for b in sorted(set(b_kern) | set(b_user))[:40]:
    print(f"  {b*5:4d}-{b*5+5:4d}s  k={b_kern[b]:6d} u={b_user[b]:6d}")

print("\n== top processes (kernel-sampled) ==")
for pid, c in proc_kern.most_common(10):
    print(f"  pid={pid:6d} {comm.get(pid,'?'):24s} k={c}")
print("== top processes (user-sampled) ==")
for pid, c in proc_user.most_common(10):
    print(f"  pid={pid:6d} {comm.get(pid,'?'):24s} u={c}")

print("\n== top user modules ==")
for m, c in user_mod.most_common(14):
    print(f"  {c:7d}  {m}")

print("\n== top user sites (module+off, group-of-16B) ==")
nm_cache = {}
def sym(mod, off):
    key = (mod, off)
    if key in nm_cache: return nm_cache[key]
    fn = None
    for cand in (os.path.join(symsdir, os.path.basename(mod)),
                 os.path.join('/tmp/ohsys', mod.lstrip('/'))):
        if os.path.exists(cand):
            try:
                out = subprocess.run(['nm', '-D', '-C', '--defined-only', cand],
                                     capture_output=True, text=True, timeout=20).stdout
                best = None
                for line in out.splitlines():
                    parts = line.split()
                    if len(parts) >= 3 and parts[0].strip('0123456789abcdef'):
                        continue
                    try: a = int(parts[0], 16)
                    except Exception: continue
                    if a <= off and (best is None or a > best[0]):
                        best = (a, parts[2])
                if best: fn = best[1] + f"+{hex(off-best[0])}"
            except Exception: pass
            break
    nm_cache[key] = fn or ''
    return nm_cache[key]

for (pid, m, off), c in user_site.most_common(18):
    print(f"  {c:6d}  pid={pid:6d} {comm.get(pid,'?'):20s} {os.path.basename(m)}+{off:#x}  {sym(m, off)}")
