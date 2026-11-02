#!/bin/bash
# r22-pipeline.sh — post-scan results pipeline.
# Usage: bash run/r22-pipeline.sh [raw-file]   (default run/results-r22-raw.txt)
set -eu
cd /home/william/rux-agents/ltp-r21
RAW="${1:-run/results-r22-raw.txt}"
OUT=results/results-r22.txt
cp "$RAW" "$OUT"
echo "=== r22 class counts ==="
awk -F'|' 'NR>1 {c[$3]++} END {for (k in c) print k, c[k]}' "$OUT" | sort
echo "=== delta vs r21 ==="
python3 run/delta.py results/results-r21.txt "$OUT" | tee results/delta-r22-vs-r21.txt
echo "=== PASS count ==="
awk -F'|' 'NR>1 && $3=="PASS"' "$OUT" | wc -l
