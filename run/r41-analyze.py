#!/usr/bin/env python3
# r41-analyze.py — post-boot metrics for the r41 continuous roll.
# Parses run/oh-<TAG>-serial.log and prints the per-boot bank row:
#   SA ceiling / insert count / last-insert time
#   marker ladder (bundle size -> SwitchToUser -> appspawn launcher -> ready)
#   unixrd phase table (t=165s window when present, else first; plus final)
#   FUX-STUCK census (line count, distinct tids, max stuck_total, comm map)
#   TXN-HANG distribution (codes histogram, max exec)
#   BWR-SNAP census (count, distinct tids)
#   pixel verdict is handled by the roller (md5 per shot).
import re, sys, collections

TAG = sys.argv[1]
LOG = f"run/oh-{TAG}-serial.log"
sa_re = re.compile(r"insert SA:(\d+)_(\d+)")
stamp_re = re.compile(r"^\[\s*([0-9.]+)\]")
sa_ids, sa_max, sa_last_t, sa_n = set(), 0, 0.0, 0
markers = []
fux_lines, fux_tids, fux_max_stuck = 0, set(), 0
tid_comm = {}
txn_codes, txn_max_exec = collections.Counter(), 0
txn_comms = collections.Counter()
bwr_n, bwr_tids = 0, set()
unixrd_first, unixrd_165, unixrd_last = {}, {}, {}
syscost_ticks = []
bwr_census_last = None
bwr_h_counts = collections.Counter()
bwr_rows = 0

for raw in open(LOG, errors="replace"):
    m = stamp_re.match(raw)
    t = float(m.group(1)) if m else -1
    line = raw[raw.find("] ") + 2:] if "] " in raw[:32] else raw
    if "insert SA:" in line:
        s = sa_re.search(line)
        if s:
            sa_n += 1
            sa_ids.add(s.group(1))
            sa_max = max(sa_max, int(s.group(2)))
            sa_last_t = t
    for mk in ("bundle size", "SwitchToUser", "appspawn start com.ohos.launcher",
               "AppSpawn start com.ohos.launcher", "bootevent.launcher.ready",
               "CreateNewUser"):
        if mk in line and "not set" not in line and (not markers or markers[-1][1] != mk):
            markers.append((t, mk))
    if line.startswith("FUX-STUCK-SUM"):
        ms = re.search(r"stuck_total=(\d+)", line)
        if ms:
            fux_max_stuck = max(fux_max_stuck, int(ms.group(1)))
    elif "FUX-STUCK" in line and "tgid=" in line:
        fux_lines += 1
        mt = re.search(r"tid=(\d+)", line)
        if mt:
            fux_tids.add(mt.group(1))
    mtc = re.search(r"TXN-HANG txn(\d+) code(\d+).*exec (\d+) ms", line)
    if mtc:
        txn_codes[mtc.group(2)] += 1
        txn_max_exec = max(txn_max_exec, int(mtc.group(3)))
        mc = re.search(r"tid (\d+) \(([^)]*)\)", line)
        if mc:
            txn_comms[mc.group(2)] += 1
            tid_comm[mc.group(1)] = mc.group(2)
        ms2 = re.search(r"tid (\d+) \(([^)]*)\)", line.split(" to ", 1)[-1])
        if ms2:
            tid_comm[ms2.group(1)] = ms2.group(2)
    if "BWR-SNAP" in line:
        bwr_n += 1
        mb = re.search(r"tid=(\d+)", line)
        if mb:
            bwr_tids.add(mb.group(1))
    if line.startswith("TXN-HANG-BWR-CENSUS"):
        bwr_census_last = line.strip()
    elif line.startswith("TXN-HANG-BWR pid="):
        bwr_rows += 1
        park = re.search(r"park=(\S+)", line)
        sysv = re.search(r"sys=(\d+)", line)
        stk = re.search(r"stack=(\d+)", line)
        sync_out = "SYNC-OUT" in line
        p = park.group(1) if park else "?"
        sv = sysv.group(1) if sysv else "?"
        sn = int(stk.group(1)) if stk else 0
        if p == "FUTEX" and sn > 0:
            bwr_h_counts["H3-futex-stack"] += 1
        elif p == "IOCTL-BWR":
            if "rsz=0" in line and re.search(r"bwr\[\w+/\1 ", line):
                bwr_h_counts["H2-write-only"] += 1
            elif sync_out:
                bwr_h_counts["H1-sync-out"] += 1
            else:
                bwr_h_counts["IOCTL-BWR-other"] += 1
        elif p == "OTHER" and sv == "16" and sync_out:
            # x86_64 decode bug (b1c7fc4c: sign-extended a1) parks BWR
            # rows as OTHER; SYNC-OUT stack + ioctl(16) = H1-approx.
            bwr_h_counts["H1-approx(signext-bug)"] += 1
        elif p == "OTHER" and sv == "16" and sn > 0:
            bwr_h_counts["IOCTL-stack-other"] += 1
    for fam in ("RM", "RF", "WV"):
        if line.startswith(f"UNIXRD-{fam} "):
            mt = re.search(r"t=(\d+)s", line)
            key = int(mt.group(1)) if mt else -1
            row = (key, line.strip())
            if fam not in unixrd_first:
                unixrd_first[fam] = row
            if abs(key - 165) <= 45:
                unixrd_165[fam] = row
            unixrd_last[fam] = row
    if "ticks(u/k/i)" in line:
        syscost_ticks.append(line.strip())

print(f"== {TAG} ==")
print(f"SA: ceiling={sa_max} inserts={sa_n} distinct_ids={len(sa_ids)} last_insert_t={sa_last_t:.1f}s")
print("ladder: " + ("; ".join(f"{tt:.0f}s {mk}" for tt, mk in markers) if markers else "NONE (never reached provisioning markers)"))
for fam in ("RM", "RF", "WV"):
    for lbl, tab in (("first", unixrd_first), ("~165s", unixrd_165), ("last", unixrd_last)):
        if fam in tab:
            print(f"UNIXRD-{fam} [{lbl} t={tab[fam][0]}s] {tab[fam][1]}")
print(f"FUX-STUCK: lines={fux_lines} distinct_tids={len(fux_tids)} max_stuck_total={fux_max_stuck}")
print(f"FUX-STUCK: tids={sorted(fux_tids)} ffrt_parked={sorted(t for t in fux_tids if tid_comm.get(t, '').startswith('OS_FFRT'))}")
print(f"TXN-HANG: n={sum(txn_codes.values())} codes={dict(txn_codes)} max_exec={txn_max_exec}ms comms={dict(txn_comms)}")
print(f"BWR-SNAP: lines={bwr_n} distinct_tids={len(bwr_tids)} (probe bwr_snap not on main; 0 expected)")
print(f"BWR-PARK: rows={bwr_rows} mix={dict(bwr_h_counts)}")
if bwr_census_last:
    print(f"BWR-PARK: {bwr_census_last}")
if syscost_ticks:
    print(f"ticks: {syscost_ticks[-1]}")
