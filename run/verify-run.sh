#!/bin/bash
# verify-run.sh <pass-label> — isolation rerun (one boot per case) of the fix-verification set.
set -u
cd /home/william/rux-agents/review-compat
PASS="${1:?a|b}"
python3 test/ltpscan/scan.py \
  --image run/rootfs-ltp-compat.img \
  --kernel run/rux-fixed-snapshot \
  --tests run/verify-cases.txt \
  --out run/verify-$PASS.txt \
  --chunk 1 --keep-logs run/verify-logs-$PASS
echo "VERIFY_${PASS}_RC=$?"
