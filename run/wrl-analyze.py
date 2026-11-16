#!/usr/bin/env python3
# wrl-analyze.py — WAKE-RUN-LEAK census anatomy + exit-owing decode for a
# wrl-class boot serial. Usage: python3 run/wrl-analyze.py run/oh-<tag>-serial.log
import re, sys
from collections import Counter, defaultdict

path = sys.argv[1]
ts_re = re.compile(r'^\[\s*(\d+\.\d+)\]')
row_re = re.compile(
    r'WAKE-RUN-LEAK tid (\d+) \(([^)]*)\) kicked (\d+)x age (\d+)s '
    r'state=(0x[0-9a-f]+) cfs_linked=(\S+) on_cpu=(\S+) pick_state=(\S+) '
    r'cpu=(\S+) pol=(\S+) on_rq=(\S+) dl_thr=(\S+) wok=(\d+) wno=(\d+) exec_ns=(\d+)')
probe_re = re.compile(r'WW-PROBE ret=(\S+) linked (\S+)->(\S+) state=(0x[0-9a-f]+)')
uiexit_re = re.compile(r'UIEXIT\[\d+\] tid=(\d+) comm=' + r"'([^']*)'" + r' tgid=(\d+).*?cause=(\S+).*?lastsys=(\d+)(?:\([^\)]*\))?.*?owing_n=(\d+) owing_txn=(\d+)(?: owing_stid=(\d+))?')
tds_re = re.compile(r'THREAD-DIED-SERVICING pid (\d+) tid (\d+) stack=(\d+) todo=(\d+) top=txn(\d+)')
rerr_re = re.compile(r'RERR-PUSH site=(\S+) dying_pid=(\d+) txn(\d+) -> (?:pid(\d+) tid(\d+)|PROC pid(\d+))')
owe_re = re.compile(r'TXN-HANG-OWE txn(\d+) tid(\d+)')
tgid_hints = defaultdict(set)  # tid -> {tgid,...}
rows = []
probes = []
uiexits = []
tds = []
rerrs = []
owes = []

for line in open(path, errors='replace'):
    ts_m = ts_re.search(line)
    ts = float(ts_m.group(1)) if ts_m else None
    m = row_re.search(line)
    if m:
        rows.append((ts,) + m.groups())
        if probes and probes[-1][0] is None:
            probes[-1] = (ts, probes[-1][1])
        continue
    p = probe_re.search(line)
    if p:
        probes.append((ts, p.groups()))
        continue
    u = uiexit_re.search(line)
    if u:
        uiexits.append((ts,) + u.groups())
        tgid_hints[int(u.group(1))].add(int(u.group(3)))
        continue
    t = tds_re.search(line)
    if t:
        tds.append((ts,) + t.groups())
        continue
    r = rerr_re.search(line)
    if r:
        rerrs.append((ts,) + r.groups())
        continue
    o = owe_re.search(line)
    if o:
        owes.append((ts,) + o.groups())
    # tid->tgid hints from other rows
    for hm in re.finditer(r'tid=(\d+)[^\n]*tgid=(\d+)', line[:200]):
        tgid_hints[int(hm.group(1))].add(int(hm.group(2)))
    for hm in re.finditer(r'tgid=(\d+)[^\n]*tid=(\d+)', line[:200]):
        tgid_hints[int(hm.group(2))].add(int(hm.group(1)))
    for hm in re.finditer(r'RWFRZ-WATCH #\d+ tid=(\d+) tgid=(\d+)', line):
        tgid_hints[int(hm.group(1))].add(int(hm.group(2)))

print(f"=== file: {path}")
print(f"WAKE-RUN-LEAK full-field rows: {len(rows)}")
tids = defaultdict(list)
for r in rows:
    tids[int(r[1])].append(r)
print(f"distinct tids: {len(tids)}")

full = rows
if full:
    print("state dist:", Counter(r[5] for r in full))
    print("on_cpu=true rows:", sum(1 for r in full if r[7] == 'true'))
    print("cfs_linked=1 rows:", sum(1 for r in full if r[6] == '1'))
    print("wok=0 rows:", sum(1 for r in full if r[13] == '0'))
    print("wok dist:", Counter(r[13] for r in full).most_common(8))
    print("wno>0 rows:", sum(1 for r in full if r[14] != '0'))
    print("exec_ns<1ms rows:", sum(1 for r in full if int(r[15]) < 1_000_000))
    print("exec_ns<10us rows:", sum(1 for r in full if int(r[15]) < 10_000))

# per-tid exec progression
adv = frozen = single = 0
frozen_list = []
for tid, rs in tids.items():
    if len(rs) < 2:
        single += 1
        continue
    execs = [int(r[15]) for r in rs]
    if max(execs) > min(execs):
        adv += 1
    else:
        frozen += 1
        frozen_list.append((tid, rs[0][2], execs[0], len(rs)))
print(f"per-tid exec: ADVANCING={adv} FROZEN={frozen} SINGLE-ROW={single}")
for t in frozen_list[:10]:
    print("  FROZEN:", t, "tgid-hints:", tgid_hints.get(t[0]))

# WW-PROBE
print("WW-PROBE:", Counter(tuple(p[1]) for p in probes).most_common(8))

# comm families
comms = Counter(r[2] for r in rows)
print("top comms:", comms.most_common(10))

# exec-tiny tids with tgid hints
tiny = sorted({int(r[1]) for r in rows if int(r[15]) < 10_000})
print(f"exec_ns<10us tids: {tiny}")
for t in tiny:
    print(f"  tid {t} comms={[r[2] for r in tids[t]]} tgid-hints={tgid_hints.get(t)} execs={[int(r[15]) for r in tids[t]]}")

# item-2 decode
print("\n=== UIEXIT rows:", len(uiexits))
owing = [u for u in uiexits if u[7] != '0']
print("UIEXIT owing_n>0:", len(owing))
for u in owing:
    print("  OWING-EXIT:", u, "tgid-hints:", tgid_hints.get(int(u[1])))
print("THREAD-DIED-SERVICING:", len(tds))
for t in tds[:20]:
    print("  TDS:", t)
print("RERR-PUSH:", len(rerrs))
for r in rerrs[:20]:
    print("  RERR:", r)
print("TXN-HANG-OWE distinct txns:", len({o[2] for o in owes}), "rows:", len(owes))
