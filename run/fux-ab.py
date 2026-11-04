#!/usr/bin/env python3
# fux-ab.py — 34th successor: A/B our fuxstuck wait-site map (wedge boot
# lb436) against the reference-kernel census (fuxref parks.tsv + maps).
# Both sides reduce to (comm, offset-in-VMA) pairs: ours from the kernel
# FUX-STUCK uvma decode, the reference from /proc/<pid>/maps lookups.
# Verdict classes per pair:
#   BOTH-LONG  — parked >100 sweeps on the healthy reference too: idle
#                FFRT worker noise, present in either kernel, NOT the
#                wedge (the boot completed around it).
#   REF-DISSOLVES — exists on the reference but its lifetime ends as
#                the boot passes the phase where ours froze: the wake
#                arrives there; ours never gets it.
#   OURS-ONLY  — no reference park at that (comm, off) at all: our
#                userspace took a branch the reference never parks on.
import collections, glob, re, sys

REFDIR = sys.argv[1] if len(sys.argv) > 1 else "run/fuxref-fx2"
OURLOG = sys.argv[2] if len(sys.argv) > 2 else "run/oh-lb436-serial.log"

# ---- reference side: parks.tsv ----
parks = collections.defaultdict(list)   # (comm,tgid,tid,uaddr) -> [sweep ts]
for ln in open(f"{REFDIR}/parks.tsv"):
    p = ln.split()
    if len(p) < 8 or p[0] != "P":
        continue
    _, t, comm, tgid, tid, uaddr, op, val = p[:8]
    parks[(comm, tgid, tid, int(uaddr, 16))].append(int(t))

# reference maps: pid -> [(start,end,perms,path)] (use the LAST snapshot;
# VMAs are stable after early boot)
maps = {}
for mf in sorted(glob.glob(f"{REFDIR}/maps-*.txt")):
    cur = None
    for ln in open(mf, errors="replace"):
        if ln.startswith("==="):
            pid = int(ln.split()[1]); cur = []
            maps[pid] = cur
            continue
        m = re.match(r"([0-9a-f]+)-([0-9a-f]+) (\S+) \S+ \S+ \S+\s*(.*)", ln)
        if m and cur is not None:
            cur.append((int(m.group(1), 16), int(m.group(2), 16), m.group(3), m.group(4).strip()))

def classify_ref(uaddr, size=0x1000):
    # anonymous ~8MB-class VMAs are the FFRT arena; return (kind, key)
    return None

# reduce reference parks to (comm, off-in-vma) with lifetime
ref_pairs = {}   # (comm, vkind, off) -> max sweeps span
for (comm, tgid, tid, uaddr), ts in parks.items():
    ts = sorted(ts)
    span = ts[-1] - ts[0] + 1
    vm = maps.get(int(tgid))
    if not vm:
        continue
    hit = None
    for s, e, perms, path in vm:
        if s <= uaddr < e:
            hit = (s, e, perms, path); break
    if not hit:
        continue
    s, e, perms, path = hit
    # only compare against the ~8MB anonymous FFRT arena class (our census
    # reported class A with ~8MB span); skip file/stack/short vm VMA records
    if path or "x" in perms or (e - s) < 0x100000:
        continue
    off = uaddr - s
    key = (comm, "A", off)
    ref_pairs[key] = max(ref_pairs.get(key, 0), span)

# ---- our side: FUX-STUCK from the wedge serial ----
pat = re.compile(
    r"FUX-STUCK(-FRESH)? tgid=(\d+) tid=(\d+) uaddr=(0x[0-9a-f]+) mm=\S+ n=\d+ "
    r"park=(\d+)s\.\.(\d+)s anywoken=(\d+) sys=\d+ epc=\S+ fop=\S+ fval=\S+ "
    r"fto=\S+(=\d+s\+\d+ns)? uvma=(0x[0-9a-f]+)-(0x[0-9a-f]+) (\S)\+(0x[0-9a-f]+)")
svc = {}
ser = open(OURLOG, errors="replace").read()
for m in re.finditer(r"ServiceStart started info ([a-zA-Z0-9_.-]+)\(pid (\d+)", ser):
    svc[m.group(2)] = m.group(1)
our_pairs = collections.Counter()
our_park = {}
for m in pat.finditer(ser):
    fresh, tgid, tid, uaddr, p1, p2, aw, ftov, vs, ve, cls, voff = m.groups()
    if cls != "A":
        continue
    comm = svc.get(tgid, f"pid{tgid}")
    key = (comm, "A", int(voff, 16))
    our_pairs[key] += 1
    our_park[key] = max(our_park.get(key, 0), int(p2))

print(f"reference pairs: {len(ref_pairs)}   our pairs: {len(our_pairs)}")
print()
rows = []
for key, cnt in our_pairs.most_common():
    comm, k, off = key
    rspan = ref_pairs.get(key)
    if rspan is None:
        # nearest reference park for the same comm (any offset)
        near = sorted((o, s) for (c, _, o), s in ref_pairs.items() if c == comm)
        verdict = "OURS-ONLY" + (f" (ref comm parks near: {near[:3]})" if near else " (ref comm NEVER parks)")
    else:
        verdict = "BOTH-LONG" if rspan > 90 else "REF-DISSOLVES"
    rows.append((cnt, comm, off, our_park[key], rspan, verdict))
for cnt, comm, off, pmax, rspan, verdict in rows:
    print(f"ours x{cnt:3d} park<={pmax:3d}s  {comm[:24]:24s} +{off:#08x}  ref_span={rspan}  {verdict}")
