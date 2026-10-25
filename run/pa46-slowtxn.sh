#!/bin/bash
# pa46-slowtxn.sh — BINDER domain check for the census boot (leg-34/35).
# Context: sweep_stale_async reclaim (de41d5ab) changed the binder
# buffer lifecycle; the gate's binder row says "queue=0 ms; image-side
# 15/30/89 s family only; no new class". REALITY CHECKS since:
#  - pa41gate (accepted leg-32 census, PRE-reclaim): an exec-side 5-8 s
#    family (code 1/64) and 10 ms queue blips are part of the recorded
#    distribution on both sides of the reclaim.
#  - pa46main/pa46f4r2 (leg-35, POST-reclaim, load1 13-17): the code-7
#    -> Init exec family is LOAD-ELASTIC — pa47smoke under light load
#    shows a tight 15.0-15.1 s cluster, while load-13-17 boots stretch
#    the same class to 18.8/26.8/59.8/60.1/77.1 s. Queue stays <=10 ms
#    in every boot pre- and post-reclaim. So exec-side totals for the
#    known classes form a continuum, not discrete families; the
#    operative regression signal stays QUEUEING:
#   FAIL conditions: (a) any queue >= 50 ms (a queueing class, the
#   thing a broken reclaim could introduce by stalling the buffer
#   pool), or (b) a total outside the known exec bands (code 1/64:
#   4.5-9 s; code 7 -> Init: 13-95 s), or (c) an unknown code.
# Usage: pa46-slowtxn.sh <serial-log>
set -u
LOG="${1:?usage: pa46-slowtxn.sh <serial-log>}"
python3 - "$LOG" <<'EOF'
import re, sys
# Known exec-side bands, keyed by binder code: code 1/64 (5-8 s
# exec, foundation->samgr cohort) and code 7 (-> Init, load-elastic
# 15 s intrinsics stretching to ~90 s under load1 13-19).
BANDS = {"1": (4_500, 8_999), "64": (4_500, 8_999), "7": (13_000, 95_000)}
rows = []
for line in open(sys.argv[1], errors="replace"):
    m = re.search(r"SLOW-TXN txn\d+ code (\d+) from pid (\d+) tid (\d+) to pid (\d+) tid (\d+): total (\d+) ms \(queue (\d+) ms, exec (\d+) ms\)", line)
    if m:
        rows.append(dict(code=m.group(1), src=m.group(2), dst=m.group(4),
                         total=int(m.group(6)), q=int(m.group(7)),
                         raw=m.group(0).strip()))
if not rows:
    print("PASS binder SLOW-TXN  n=0  (no slow txn at all)")
    raise SystemExit
bad_q = [r for r in rows if r["q"] >= 50]
unknown = [r for r in rows
           if r["code"] not in BANDS
           or not (BANDS[r["code"]][0] <= r["total"] <= BANDS[r["code"]][1])]
verdict = "FAIL" if (bad_q or unknown) else "PASS"
print(f"{verdict} binder SLOW-TXN  n={len(rows)}  queue>=50ms={len(bad_q)}  out-of-band={len(unknown)}")
from collections import Counter
c = Counter()
for r in rows:
    lo, hi = BANDS.get(r["code"], (0, 0))
    inband = lo <= r["total"] <= hi
    c["code%s %s" % (r["code"], "in-band" if inband else "OUT(%dms)" % r["total"])] += 1
print("  classes:", dict(c))
print("  max queue ms:", max(r["q"] for r in rows))
for r in (bad_q + unknown)[:5]:
    print("  offending:", r["raw"][:140])
EOF
