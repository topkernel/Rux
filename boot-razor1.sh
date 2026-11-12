#!/bin/bash
# razor1: the three-way verdict boot — insert_dirty failures + per-sync
# entry census for the pattern file (pristine image, wedge-fixed kernel).
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor1.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor1 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor1-driver.out 2>&1
