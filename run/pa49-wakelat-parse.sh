#!/bin/bash
# pa49-wakelat-parse.sh — host-side parse of the pa49 wake-latency
# capture. Reads the serial log between ===WL-DUMP === and ===WL-END,
# pairs sched_waking/sched_wakeup with the next sched_switch that
# schedules the woken pid in, and prints the latency distributions.
#   wake->run(waking)  = sched_waking ts -> sched_switch(next=pid) ts
#   wake->run(wakeup)  = sched_wakeup ts -> the same switch
# The second removes the enqueue-attempt cost; the delta between the
# two is the enqueue-side wait. A waking evicted before its switch (or
# a switch whose waking was evicted) shows up as unmatched and is
# counted, not silently dropped.
# Usage: pa49-wakelat-parse.sh <serial-log>
set -u
python3 - "${1:?usage: pa49-wakelat-parse.sh <serial-log>}" <<'EOF'
import re, sys
log = open(sys.argv[1], errors="replace").read()
m = re.search(r"===WL-DUMP ===\n(.*?)===WL-END", log, re.S)
if not m:
    print("no WL-DUMP section found"); sys.exit(1)
stats = re.findall(r"===WL-STATS cpu(\d) ===.*?(entries: \d+\noverrun: \d+.*?read events: \d+)", log, re.S)
for cpu, txt in stats:
    ov = re.search(r"overrun: (\d+)", txt)
    rd = re.search(r"read events: (\d+)", txt)
    en = re.search(r"entries: (\d+)", txt)
    print(f"ring cpu{cpu}: entries={en.group(1) if en else '?'} overrun={ov.group(1) if ov else '?'} read_events={rd.group(1) if rd else '?'}")

line_re = re.compile(
    r"^\s*(\S+)-(\d+)\s+\[(\d+)\]\s+\S{5}\s+(\d+)\.(\d{6}):\s+(\w+):\s+(.*)$")
wak_re = re.compile(r"comm=\S+ pid=(\d+) prio=\d+ target_cpu=(\d+)")
sw_re = re.compile(r"prev_comm=\S+ prev_pid=(\d+) .*==> next_comm=\S+ next_pid=(\d+)")

lat_waking, lat_wakeup = [], []
last_waking, last_wakeup = {}, {}
unmatched_switch, ev = 0, 0
for line in m.group(1).splitlines():
    mm = line_re.match(line)
    if not mm:
        continue
    ev += 1
    ts = int(mm.group(4)) * 1_000_000 + int(mm.group(5))
    name, args = mm.group(6), mm.group(7)
    if name == "sched_waking":
        w = wak_re.match(args)
        if w and w.group(1) != "0":
            last_waking[int(w.group(1))] = ts
    elif name == "sched_wakeup":
        w = wak_re.match(args)
        if w and w.group(1) != "0":
            last_wakeup[int(w.group(1))] = ts
    elif name == "sched_switch":
        s = sw_re.match(args)
        if not s:
            continue
        nxt = int(s.group(2))
        if nxt == 0:
            continue
        t1 = last_waking.pop(nxt, None)
        t2 = last_wakeup.pop(nxt, None)
        if t1 is not None and ts >= t1:
            lat_waking.append(ts - t1)
        else:
            unmatched_switch += 1
        if t2 is not None and ts >= t2:
            lat_wakeup.append(ts - t2)

def dist(vals, label):
    if not vals:
        print(f"{label}: no samples"); return
    vals.sort()
    n = len(vals)
    q = lambda p: vals[min(n - 1, int(p * n))]
    us = lambda v: f"{v:.1f}us" if v < 1000 else f"{v/1000:.2f}ms"
    print(f"{label}: n={n} p50={us(q(.5))} p90={us(q(.9))} p99={us(q(.99))} max={us(vals[-1])}")
    buckets = [(0,10),(10,25),(25,50),(50,100),(100,250),(250,500),(500,1000),(1000,5000),(5000,10**9)]
    labels = ["<10us","10-25","25-50","50-100","100-250","250-500","0.5-1ms","1-5ms",">5ms"]
    hist = [0]*len(buckets)
    for v in vals:
        for i,(lo,hi) in enumerate(buckets):
            if lo <= v < hi:
                hist[i] += 1; break
    bar = max(1, max(hist))
    for i,(lab) in enumerate(labels):
        print(f"  {lab:>8} {hist[i]:>7} {'#'*(hist[i]*40//bar)}")

print(f"events parsed: {ev}, matched waking->switch: {len(lat_waking)}, "
      f"wakeup->switch: {len(lat_wakeup)}, unmatched switches (eviction/preempt-rematch): {unmatched_switch}")
dist(lat_waking, "wake->run (from sched_waking, includes enqueue)")
dist(lat_wakeup, "wake->run (from sched_wakeup, post-enqueue)")
EOF
