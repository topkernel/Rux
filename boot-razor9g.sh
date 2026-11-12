#!/bin/bash
# razor9g: extent-insert divergence tracer + corrected phase probe
# (stall hook removed) — pins where the map shifts +S despite identity
# upstream. Plus the virtio backpressure fix (9c-class kernel).
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor9g.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor9g 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor9g-driver.out 2>&1
