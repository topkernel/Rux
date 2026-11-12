#!/bin/bash
# fvpop2 (109th): POPULATION boot with the R108 kernel (crash-ordered
# frees: shrunk/erased map durable BEFORE bitmap free) from the true
# pristine (oh-launcher's ud-f2-u2v15.img). Produces a populated base
# whose map-vs-bitmap state carries only R108's benign leak direction.
# The crash script no-ops on the never-booted base (no /local/tmp yet).
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-fvpop2.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py fvpop2 420 run/system-fiF.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/fvpop2-driver.out 2>&1
