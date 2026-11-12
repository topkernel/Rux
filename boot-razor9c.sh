#!/bin/bash
# razor9c: v5 ledger + chunk-stall regime hook + virtio mass-EIO
# backpressure fix — acceptance candidate #1 on the fixed kernel.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor9c.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor9c 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor9c-driver.out 2>&1
