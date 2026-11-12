#!/bin/bash
# razor10b: acceptance boot #1 — populated-pristine base (razor10 served
# as the population boot: restored never-booted image + full first-boot
# population, pat.bin never existed), fixed kernel, hpout drive restored.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor10b.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor10b 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor10b-driver.out 2>&1
