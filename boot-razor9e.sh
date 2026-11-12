#!/bin/bash
# razor9e: v5 ledger + deepened chunk-stall regime, second sample.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor9e.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor9e 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor9e-driver.out 2>&1
