#!/bin/bash
# iso21-run.sh — two isolation passes (a, b) over the r21 losses+gains.
set -u
cd /home/william/rux-agents/ltp-r21
for pass in a b; do
  echo "=== ISO21 pass $pass start $(date +%s)" 
  python3 test/ltpscan/scan-x86_64.py \
    --image test/rootfs-x86_64-ltp.img \
    --kernel target/x86_64-unknown-none/debug/bzImage \
    --tests run/iso-all.txt \
    --out run/iso21-$pass.txt \
    --chunk 100 --keep-logs run/iso21-logs-$pass
  echo "ISO21 pass $pass rc=$?"
done
echo "ISO21 ALL DONE"
