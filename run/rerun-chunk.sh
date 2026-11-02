#!/bin/bash
# rerun-chunk.sh — re-run ONE chunk (test index range) of the 1869 list into
# its own results file, for splicing after a kernel fix (chunk@700 panic).
# Usage: bash run/rerun-chunk.sh <start_idx> <end_idx> <tag>
# Example: bash run/rerun-chunk.sh 700 799 r22c7
set -eu
cd /home/william/rux-agents/ltp-r21
S="${1:?start idx}"; E="${2:?end idx}"; TAG="${3:?tag}"
sed -n "$((S+1)),$((E+1))p" test/ltpscan/tests-all.txt > "run/chunk-$TAG.txt"
mkdir -p "run/iso22-logs-$TAG"
python3 test/ltpscan/scan-x86_64.py \
  --image test/rootfs-x86_64-ltp.img \
  --kernel target/x86_64-unknown-none/debug/bzImage \
  --tests "run/chunk-$TAG.txt" \
  --out "run/results-$TAG-raw.txt" \
  --chunk 100 --keep-logs "run/iso22-logs-$TAG"
echo "RERUN $TAG DONE"
