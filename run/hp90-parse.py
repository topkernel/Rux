#!/usr/bin/env python3
# hiperf .data walker v2 (90th) with zero-hole resync: guest ext4
# block-loss leaves zero runs inside the data section (observed on the
# 89th's file: 8MB hole), so we (a) read the header for attr/sample_type,
# (b) if the attr region is clobbered too, fall back to probing a sample
# record for its size, (c) walk record chains, resyncing at the first
# plausible header after a break, and report covered ranges.
import struct, sys, bisect, collections

path, syms_file = sys.argv[1], (sys.argv[2] if len(sys.argv) > 2 else None)
out_prefix = sys.argv[3] if len(sys.argv) > 3 else "hp"

d = open(path, "rb").read()
(h_size, attr_size, attrs_off, attrs_sz, data_off, data_sz,
 et_off, et_sz) = struct.unpack_from("<QQQQQQQQ", d, 8)
data_end = min(data_off + data_sz, len(d))
print(f"attrs@{attrs_off:#x}+{attrs_sz:#x} data@{data_off:#x}+{data_sz:#x} "
      f"file={len(d):#x} data_end={data_end:#x}")

ST = 0
attr_ok = False
try:
    cand = struct.unpack_from("<Q", d, attrs_off + 24)[0]
    if cand and (cand & ~0x8000_01FF) == 0:  # only known low bits + bit31
        ST = cand
        attr_ok = True
except struct.error:
    pass
S_IP, S_TID, S_TIME, S_ADDR, S_ID, S_SID, S_CPU, S_PERIOD = 1, 2, 4, 8, 16, 32, 64, 128
S_CALLCHAIN, S_IDENTIFIER = 512, 1 << 16

def sample_size(st):
    n = 8
    if st & S_IDENTIFIER: n += 8
    if st & S_IP: n += 8
    if st & S_TID: n += 8
    if st & S_TIME: n += 8
    if st & S_ADDR: n += 8
    if st & S_ID: n += 8
    if st & S_SID: n += 8
    if st & S_CPU: n += 16
    if st & S_PERIOD: n += 8
    return n

# kernel symbol table (nm output or addr<space>name)
syms, sym_addrs = [], []
if syms_file:
    for line in open(syms_file):
        parts = line.split(None, 1)
        if len(parts) == 2:
            try:
                syms.append((int(parts[0], 16), parts[1].strip()))
            except ValueError:
                pass
    syms.sort()
    sym_addrs = [a for a, _ in syms]

def ksym(ip):
    i = bisect.bisect_right(sym_addrs, ip) - 1
    return f"{syms[i][1]}+{ip - syms[i][0]:#x}" if i >= 0 else "?"

def parse_sample_probe(p, st):
    ip = pid = 0
    if st & S_IDENTIFIER: p += 8
    if st & S_IP:
        ip = struct.unpack_from("<Q", d, p)[0]; p += 8
    if st & S_TID:
        pid, _tid = struct.unpack_from("<ii", d, p); p += 8
    return ip, pid

def parse_sample(p, end, misc):
    ip, pid = parse_sample_probe(p, ST)
    return ip, pid, misc

def plausible(off, depth=8):
    """check a chain of `depth` records starting at off"""
    o = off
    for _ in range(depth):
        if o + 8 > data_end: return False
        t, misc, sz = struct.unpack_from("<IHH", d, o)
        if sz < 8 or sz % 4 or o + sz > data_end: return False
        if t > 0x7ffffff or (t not in (1,2,3,4,5,6,7,8,9,12,13,70,71,74,79)
                             and not (0x7ffffff0 <= t <= 0x7fffffff)): return False
        if t == 9 and not (misc in (1, 2) and 0x30 <= sz <= 0x8000): return False
        o += sz
    return True

# probe the real sample layout: score candidate STs across up to 200
# sample records; the winner must parse sane pid/ip on nearly all.
def collect_sample_offsets(limit=200):
    offs = []
    o = data_off
    while o + 8 <= data_end and len(offs) < limit:
        t, misc, sz = struct.unpack_from("<IHH", d, o)
        if t == 9 and misc in (1, 2, 3) and 0x28 <= sz <= 0x200 and o + sz <= data_end:
            offs.append(o)
            o += sz
        else:
            o += 4
    return offs

cand_sts = [c for c in dict.fromkeys(
    [ST, 0xF7, 0x100F7, ST | S_IDENTIFIER, 0xEF, 0x10EF, 0xD7, 0x100D7,
     0x2F7, 0x30F7, 0x100D7 | 512, 0x100F7 | 512])]
soffs = collect_sample_offsets()
best_st, best_score = ST or 0xF7, -1.0
for st_c in cand_sts:
    if not st_c:
        continue
    fixed = sample_size(st_c)
    okn = tot = 0
    for o in soffs:
        sz = struct.unpack_from("<H", d, o + 6)[0]
        if sz != fixed and not (st_c & 512 and sz > fixed):
            tot += 1
            continue
        ip, pid = parse_sample_probe(o + 8, st_c)
        tot += 1
        if 0 <= pid < 30000 and (ip >= 0xFFFFFFFF80000000 or ip < 0x0000800000000000):
            okn += 1
    score = okn / max(tot, 1)
    if score > best_score:
        best_st, best_score = st_c, score
print(f"sample_type probe: {len(soffs)} recs, best ST={best_st:#x} score={best_score:.2f}")
ST = best_st

off = data_off
proc = collections.defaultdict(lambda: [0, 0])
proc_comm = {}
kern_hist = collections.Counter()
user_hist = collections.Counter()
mmaps = collections.defaultdict(list)
n_records = collections.Counter()
samples = 0
resyncs = 0
covered = 0
S_CALLCHAIN = 512
chain_hist = collections.defaultdict(collections.Counter)  # leaf ip -> caller counter
chain_depth = collections.Counter()

def parse_chain(p):
    """return list of ips if CALLCHAIN present in ST, else None"""
    if not (ST & S_CALLCHAIN):
        return None
    # walk the fixed fields to reach the callchain nr (order matters)
    q = p
    if ST & S_IDENTIFIER: q += 8
    if ST & S_IP: q += 8
    if ST & S_TID: q += 8
    if ST & S_TIME: q += 8
    if ST & S_ADDR: q += 8
    if ST & S_ID: q += 8
    if ST & S_SID: q += 8
    if ST & S_CPU: q += 16
    if ST & S_PERIOD: q += 8
    nr = struct.unpack_from("<Q", d, q)[0]
    if nr > 128:
        return None
    return list(struct.unpack_from(f"<{nr}Q", d, q + 8))

# main walk with resync: if header implausible, scan forward 4 bytes
# at a time for the next plausible chain start
while off + 8 <= data_end:
    t, misc, sz = struct.unpack_from("<IHH", d, off)
    if sz < 8 or sz % 4 or off + sz > data_end or (
            t not in (1, 2, 3, 4, 5, 6, 7, 8, 9, 12, 13, 70, 71, 74, 79)
            and not (0x7ffffff0 <= t <= 0x7fffffff)):
        off += 4
        resyncs += 1
        continue
    p = off + 8
    covered += sz
    n_records[t] += 1
    try:
        if t == 9:
            ip, pid, _m = parse_sample(p, off + sz, misc)
            samples += 1
            chain = parse_chain(p)
            if ip >= 0xFFFFFFFF80000000:
                proc[pid][0] += 1
                kern_hist[ip] += 1
                if chain and len(chain) > 1:
                    chain_depth[len(chain)] += 1
                    chain_hist[ip][chain[1]] += 1
            else:
                proc[pid][1] += 1
                user_hist[(pid, ip)] += 1
        elif t == 1:
            pid, _tid, start, ln, pgoff = struct.unpack_from("<IIQQQ", d, p)
            name = d[p+32:off+sz].split(b"\0")[0].decode("utf-8", "replace")
            mmaps[pid].append((start, start + ln, pgoff, name))
        elif t == 12:
            pid, _tid, start, ln, pgoff = struct.unpack_from("<IIQQQ", d, p)
            name = d[p+64:off+sz].split(b"\0")[0].decode("utf-8", "replace")
            mmaps[pid].append((start, start + ln, pgoff, name))
        elif t == 3:
            pid, tid = struct.unpack_from("<ii", d, p)
            if pid == tid:
                proc_comm[pid] = d[p+8:off+sz].split(b"\0")[0].decode("utf-8", "replace")
    except struct.error:
        pass
    off += sz

print(f"sample_type={ST:#x} attr_ok={attr_ok} records={dict(n_records)} "
      f"samples={samples} resync-bytes={resyncs} covered={covered/data_sz:.0%}")
print(f"mmap recs pid-count={len(mmaps)} comm-count={len(proc_comm)}")

def comm(tgid):
    return proc_comm.get(tgid, "?")

with open(f"{out_prefix}-procs.txt", "w") as w:
    w.write("tgid comm kern user total\n")
    for tgid, (k, u) in sorted(proc.items(), key=lambda kv: -(kv[1][0]+kv[1][1])):
        w.write(f"{tgid} {comm(tgid)} {k} {u} {k+u}\n")

with open(f"{out_prefix}-kernel-top.txt", "w") as w:
    tot = sum(kern_hist.values())
    w.write(f"# kernel samples {tot}\ncount pct addr symbol\n")
    for ip, c in kern_hist.most_common(150):
        w.write(f"{c} {100.0*c/max(tot,1):.2f}% {ip:#x} {ksym(ip)}\n")

# caller attribution for the top kernel leaves (fp records)
if chain_hist:
    with open(f"{out_prefix}-callers.txt", "w") as w:
        for ip, c in kern_hist.most_common(25):
            if ip not in chain_hist:
                continue
            w.write(f"LEAF {c} {ksym(ip)}\n")
            for caller, cc in chain_hist[ip].most_common(12):
                w.write(f"    {cc} <- {ksym(caller)}\n")
    print("chain depths:", dict(sorted(chain_depth.items())[:10]))
    print("callers ->", f"{out_prefix}-callers.txt")

def module_of(tgid, ip):
    for (s, e, pgoff, name) in mmaps.get(tgid, ()):
        if s <= ip < e:
            return f"{name}+{pgoff + ip - s:#x}"
    return "?nommap"

with open(f"{out_prefix}-user-top.txt", "w") as w:
    tot = sum(user_hist.values())
    w.write(f"# user samples {tot}\ncount pct tgid comm module+off\n")
    for (tgid, ip), c in user_hist.most_common(200):
        w.write(f"{c} {100.0*c/max(tot,1):.2f}% {tgid} {comm(tgid)} {module_of(tgid, ip)}\n")

mod_hist = collections.Counter()
for (tgid, ip), c in user_hist.items():
    mod_hist[(tgid, comm(tgid), module_of(tgid, ip))] += c
with open(f"{out_prefix}-user-mods.txt", "w") as w:
    for (tgid, c_, mod), c in mod_hist.most_common(250):
        w.write(f"{c} {tgid} {c_} {mod}\n")

print("wrote", out_prefix + "-{procs,kernel-top,user-top,user-mods}.txt")
