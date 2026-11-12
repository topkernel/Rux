#!/bin/bash
# razor9d: v5 ledger + deepened chunk-stall regime (print + 1ms stall)
# + virtio mass-EIO backpressure fix.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor9d.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor9d 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor9d-driver.out 2>&1
