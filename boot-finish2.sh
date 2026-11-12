#!/bin/bash
# finish1: THE P0 acceptance, phase A — pristine userdata, pattern write,
# sync, hard crash. Kernel = wedge-fixed + instruments.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-finish2.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py finish2 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/finish2-driver.out 2>&1
