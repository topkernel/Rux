#!/bin/bash
# r24-pipeline.sh — post-scan results pipeline.
# Usage: bash run/r24-pipeline.sh [raw-file]   (default run/results-r24-raw.txt)
set -eu
cd /home/william/rux-agents/ltp-r21
RAW="${1:-run/results-r24-raw.txt}"
OUT=results/results-r24.txt
cp "$RAW" "$OUT"
echo "=== r24 class counts ==="
awk -F'|' 'NR>1 {c[$3]++} END {for (k in c) print k, c[k]}' "$OUT" | sort
echo "=== delta vs r23 ==="
python3 run/delta.py results/results-r23.txt "$OUT" | tee results/delta-r24-vs-r23.txt
echo "=== PASS count ==="
awk -F'|' 'NR>1 && $3=="PASS"' "$OUT" | wc -l
