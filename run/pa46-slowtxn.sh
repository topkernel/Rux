#!/bin/bash
# pa46-slowtxn.sh — BINDER domain check for the census boot (leg-34).
# Context: sweep_stale_async reclaim (de41d5ab) changed the binder
# buffer lifecycle; the gate's binder row says "queue=0 ms; image-side
# 15/30/89 s family only; no new class". REALITY CHECK from pa41gate
# (the accepted leg-32 census, PRE-reclaim kernel): it already shows an
# exec-side 5-8 s family (code 1, foundation->samgr cohort) and one
# 10 ms queue blip — so the doc's family list is stale, and the
# operative check is NO NEW CLASS vs the recorded distribution:
#   FAIL conditions: (a) any queue >= 50 ms (a queueing class, the
#   thing a broken reclaim could introduce by stalling the buffer
#   pool), or (b) totals outside every known family (5-8, 15, 30, 89 s).
# Usage: pa46-slowtxn.sh <serial-log>
set -u
LOG="${1:?usage: pa46-slowtxn.sh <serial-log>}"
python3 - "$LOG" <<'EOF'
import re, sys
FAMS = [(4_500, 8_999, "5-8s exec"), (13_000, 17_000, "15s"),
        (27_000, 33_000, "30s"), (80_000, 95_000, "89s")]
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
unknown = [r for r in rows if not any(lo <= r["total"] <= hi for lo, hi, _ in FAMS)]
verdict = "FAIL" if (bad_q or unknown) else "PASS"
print(f"{verdict} binder SLOW-TXN  n={len(rows)}  queue>=50ms={len(bad_q)}  out-of-family={len(unknown)}")
from collections import Counter
c = Counter()
for r in rows:
    fam = next((n for lo, hi, n in FAMS if lo <= r["total"] <= hi),
               "NEW(%dms)" % r["total"])
    c["%s code%s" % (fam, r["code"])] += 1
print("  classes:", dict(c))
print("  max queue ms:", max(r["q"] for r in rows))
for r in (bad_q + unknown)[:5]:
    print("  offending:", r["raw"][:140])
EOF
