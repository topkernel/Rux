#!/bin/bash
# razor9b: v5 ledger, second sample — need a ring-saturation boot
# (razor9 booted too fast: occupancy never exceeded 180 bytes).
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor9b.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor9b 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor9b-driver.out 2>&1
