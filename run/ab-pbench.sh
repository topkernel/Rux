#!/bin/bash
# ab-pbench.sh — interleaved same-rig pbench A/B for the perf-gap-hunt
# lane (merge-gate Tier 2 evidence: mm-hot-path changes must hold
# P1-P7 medians within ±20% of the base column or improve).
# Usage: ab-pbench.sh <bzImage-A> <bzImage-B> <label-A> <label-B> <reps>
# Output: run/abpb-<A>-<B>.txt with per-rep PBENCH rows, plus a median
# compare at the end (python3).
set -u
KA="$1"; KB="$2"; LA="$3"; LB="$4"; REPS="$5"
WT="/home/william/rux-agents/perf-gap-hunt"
OUT="$WT/run/abpb-$LA-$LB.txt"
: > "$OUT"
for rep in $(seq 1 "$REPS"); do
    for side in A B; do
        k="$KA"; lbl="$LA"
        [ "$side" = B ] && { k="$KB"; lbl="$LB"; }
        echo "===PB $lbl rep$rep $(date +%H:%M:%S) load=$(cut -d' ' -f1 /proc/loadavg)" >> "$OUT"
        python3 "$WT/test/pbench_boot.py" --bzimage "$k" --tag "abpb-$lbl-$rep" \
            --accel kvm --smp 2 --mem 1G >> "$OUT" 2>&1
        echo "===END $lbl rep$rep rc=$?" >> "$OUT"
    done
done
# Median compare per row across reps (medians only; p99 is host noise).
python3 - "$OUT" "$LA" "$LB" <<'EOF'
import re, statistics, sys
path, la, lb = sys.argv[1], sys.argv[2], sys.argv[3]
rows = {}
cur = None
for line in open(path, errors="replace"):
    m = re.match(r"===PB (\S+) rep(\d+)", line)
    if m:
        cur = m.group(1); continue
    m = re.match(r"PBENCH (\S+) n=\d+ med=([0-9.]+)", line)
    if m and cur:
        rows.setdefault(m.group(1), {}).setdefault(cur, []).append(float(m.group(2)))
print(f"{'row':28s} {la:>12s} {lb:>12s}  delta%")
for r in sorted(rows):
    a = rows[r].get(la, []); b = rows[r].get(lb, [])
    if not a or not b:
        print(f"{r:28s} MISSING ({len(a)}/{len(b)} reps)"); continue
    ma, mb = statistics.median(a), statistics.median(b)
    d = (mb - ma) / ma * 100 if ma else 0
    flag = "REGRESS>20%" if d > 20 else ""
    print(f"{r:28s} {ma:12.3f} {mb:12.3f}  {d:+.1f}% {flag}")
EOF
echo "ab-pbench done -> $OUT"
