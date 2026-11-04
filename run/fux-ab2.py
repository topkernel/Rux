#!/usr/bin/env python3
# fux-ab2.py — 34th successor: refined A/B with the fast (sub-second)
# reference census. For every (comm, offset-in-8MB-anon-arena) pair our
# wedge census reported, classify against the reference's park EPISODES:
#   REF-NEVER  — no park (even >=100ms) at that pair: our userspace
#                waits on an object the reference never waits on ->
#                path divergence (BMSSCAN-style branch hunt).
#   REF-BRIEF  — parks exist but episodes are short (woken): the wake
#                arrives on the reference -> missing kernel-side event
#                delivery on ours.
#   REF-CYCLES — parks exist with long/timeout-cycled episodes: idle
#                poll cycles, benign in both kernels.
import collections, glob, re, sys

REFDIR = sys.argv[1] if len(sys.argv) > 1 else "run/fuxref-fx5"
OURLOGS = sys.argv[2:] if len(sys.argv) > 2 else ["run/oh-lb436-serial.log", "run/oh-lb389-serial.log"]

# ---- reference: fast.tsv episodes ----
# line: F <uptime> <idle> comm tgid tid uaddr op val   (cut -d. -f1-2 of
# /proc/uptime yields TWO tokens: uptime and idle)
raw = []
for ln in open(f"{REFDIR}/fast.tsv", errors="replace"):
    p = ln.split()
    if len(p) < 9 or p[0] != "F":
        continue
    raw.append((float(p[1]), p[3], p[4], p[5], int(p[6], 16)))

# maps: list of snapshots (time-ordered by filename int)
snaps = []
for mf in sorted(glob.glob(f"{REFDIR}/maps-*.txt"), key=lambda s: int(re.search(r"(\d+)", s).group(1))):
    pid = None
    m = {}
    for ln in open(mf, errors="replace"):
        if ln.startswith("==="):
            pid = int(ln.split()[1]); m[pid] = []
            continue
        mm = re.match(r"([0-9a-f]+)-([0-9a-f]+) (\S+) \S+ \S+ \S+\s*(.*)", ln)
        if mm and pid is not None:
            m[pid].append((int(mm.group(1), 16), int(mm.group(2), 16), mm.group(3), mm.group(4).strip()))
    snaps.append(m)

def vma_off(tgid, uaddr):
    for m in snaps:
        for s, e, perms, path in m.get(int(tgid), []):
            if s <= uaddr < e and not path and "x" not in perms and (e - s) >= 0x100000:
                return uaddr - s
    return None

# episodes: same (tgid,tid,uaddr) sampled at ts list; gaps > 2s split
episodes = collections.defaultdict(list)
for ts, comm, tgid, tid, uaddr in raw:
    episodes[(comm, tgid, tid, uaddr)].append(ts)
ref_pairs = collections.defaultdict(list)   # (comm, off) -> [episode durations]
for (comm, tgid, tid, uaddr), ts in episodes.items():
    ts.sort()
    start = prev = ts[0]
    for t in ts[1:]:
        if t - prev > 2.0:
            ref_pairs.setdefault((comm, vma_off(tgid, uaddr)), []).append(prev - start)
            start = t
        prev = t
    ref_pairs.setdefault((comm, vma_off(tgid, uaddr)), []).append(prev - start)

# ---- our side: FUX-STUCK pairs from the wedge serials ----
pat = re.compile(
    r"FUX-STUCK(-FRESH)? tgid=(\d+) tid=(\d+) uaddr=(0x[0-9a-f]+) mm=\S+ n=\d+ "
    r"park=(\d+)s\.\.(\d+)s anywoken=(\d+) sys=\d+ epc=\S+ fop=\S+ fval=\S+ "
    r"fto=\S+(=(\d+)s\+(\d+)ns)? uvma=(0x[0-9a-f]+)-(0x[0-9a-f]+) (\S)\+(0x[0-9a-f]+)")
our = collections.Counter()
our_max = {}
our_tmo = {}
svc = {}
for OURLOG in OURLOGS:
    ser = open(OURLOG, errors="replace").read()
    for m2 in re.finditer(r"ServiceStart started info ([a-zA-Z0-9_.-]+)\(pid (\d+)", ser):
        svc[m2.group(2)] = m2.group(1)
    for m in pat.finditer(ser):
        g = m.groups()
        # 0 fresh, 1 tgid, 2 tid, 3 uaddr, 4 p1, 5 p2, 6 anywoken,
        # 7 fto-group, 8 ftosec, 9 ftonsec, 10 vstart, 11 vend,
        # 12 cls, 13 voff
        tgid, p2, ftosec, cls, voff = g[1], g[5], g[8], g[12], g[13]
        if cls != "A":
            continue
        comm = svc.get(tgid, f"pid{tgid}")
        key = (comm, int(voff, 16))
        our[key] += 1
        our_max[key] = max(our_max.get(key, 0), int(p2))
        if ftosec is not None:
            our_tmo[key] = f"{ftosec}s"

print(f"ref fast episodes: {sum(len(v) for v in ref_pairs.values())} "
      f"across {len(ref_pairs)} pairs; our pairs: {len(our)}")
print()
print(f"{'count':>5} {'park<=':>6}  {'comm':24s} {'off':>10} {'tmo':>10}  ref verdict")
for key, cnt in our.most_common():
    comm, off = key
    eps = ref_pairs.get(key)
    if eps is None:
        near = sorted(o for (c, o), _ in ref_pairs.items() if c == comm and o is not None)
        verdict = "REF-NEVER" + (f" (comm offs: {[hex(o) for o in near[:5]]})" if near else " (comm absent)")
    else:
        mx = max(eps)
        verdict = f"REF-CYCLES max={mx:.0f}s n={len(eps)}" if mx > 30 else f"REF-BRIEF max={mx:.1f}s n={len(eps)}"
    print(f"x{cnt:4d} {our_max.get(key,0):5d}s  {comm[:24]:24s} {off:#10x} {our_tmo.get(key,'?'):>10}  {verdict}")
