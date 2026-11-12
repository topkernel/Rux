#!/bin/bash
# fv1: acceptance boot #1 after the R12 bio eviction-window fix —
# populated-pristine base (razor10's boot image, pat.bin-free), fixed
# kernel, hpout drive present.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-fv1.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py fv1 420 run/system-fiF.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/fv1-driver.out 2>&1
