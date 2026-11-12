#!/bin/bash
# fvpop: POPULATION boot with the FIXED kernel (R107 stale-map erase +
# R12 bio eviction protocol) from the never-booted pristine
# (oh-launcher's ud-f2-u2v15.img). The crash script no-ops on the
# never-booted base (no /local/tmp yet); this boot exists to produce a
# CLEAN POPULATED base whose map-vs-bitmap state was produced by the
# fixed kernel (the razor10 base carried ino-413-class stale maps from
# the pre-fix population boot — the poison the 107th measured).
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-fvpop.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py fvpop 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/fvpop-driver.out 2>&1
