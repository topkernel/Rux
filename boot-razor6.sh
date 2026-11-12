#!/bin/bash
# razor6: the presence-transition boot — 1Hz FI-WATCH slot-table dump +
# keyed cap raises (invalidate/remove/evict/flush). Pristine userdata;
# the second the pattern entry vanishes names the actor.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor6.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor6 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor6-driver.out 2>&1
