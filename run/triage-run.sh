#!/bin/bash
# Baseline triage: one boot per case, pass A only (triage), on this worktree's kernel.
set -u
cd /home/william/rux-agents/review-compat
python3 test/ltpscan/scan.py \
  --image run/rootfs-ltp-compat.img \
  --kernel target/riscv64gc-unknown-none-elf/debug/rux \
  --tests run/triage-cases.txt \
  --out run/triage-baseline.txt \
  --chunk 1 --keep-logs run/triage-logs
echo "TRIAGE_RC=$?"
