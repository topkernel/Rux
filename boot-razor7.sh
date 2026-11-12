#!/bin/bash
# razor7: the overlap-shape boot with the phase probes — FI-PHASE-MISMATCH
# names the layer (buffered-src / blockwrite / flush-src) where page data
# gets cross-wired during the write/sync overlap; FI-ALLOC logs writeback
# materialization placements.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor7.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor7 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor7-driver.out 2>&1
