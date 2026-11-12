#!/bin/bash
# razor9: pipe ring-continuity ledger (v5) — occupancy + stream-phase
# checks under the pipe lock pin the side of the 8-byte-per-chunk skip.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor9.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor9 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor9-driver.out 2>&1
