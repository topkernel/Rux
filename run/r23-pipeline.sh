#!/bin/bash
# r23-pipeline.sh — post-scan results pipeline.
# Usage: bash run/r23-pipeline.sh [raw-file]   (default run/results-r23-raw.txt)
set -eu
cd /home/william/rux-agents/ltp-r21
RAW="${1:-run/results-r23-raw.txt}"
OUT=results/results-r23.txt
cp "$RAW" "$OUT"
echo "=== r23 class counts ==="
awk -F'|' 'NR>1 {c[$3]++} END {for (k in c) print k, c[k]}' "$OUT" | sort
echo "=== delta vs r22 ==="
python3 run/delta.py results/results-r22.txt "$OUT" | tee results/delta-r23-vs-r22.txt
echo "=== PASS count ==="
awk -F'|' 'NR>1 && $3=="PASS"' "$OUT" | wc -l
