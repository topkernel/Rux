#!/bin/bash
# r27-pipeline.sh — post-scan results pipeline.
# Usage: bash run/r27-pipeline.sh [raw-file]   (default run/results-r27-raw.txt)
set -eu
cd /home/william/rux-agents/ltp-r27
RAW="${1:-run/results-r27-raw.txt}"
OUT=results/results-r27.txt
cp "$RAW" "$OUT"
echo "=== r27 class counts ==="
awk -F'|' 'NR>1 {c[$3]++} END {for (k in c) print k, c[k]}' "$OUT" | sort
echo "=== delta vs r26 ==="
python3 run/delta.py results/results-r26.txt "$OUT" | tee results/delta-r27-vs-r26.txt
echo "=== PASS count ==="
awk -F'|' 'NR>1 && $3=="PASS"' "$OUT" | wc -l
