#!/bin/bash
# razor2: target-keyed probes — uncapped insert-fail totals in the
# summary + keyed per-512 prints. Pristine, wedge-fixed, full chain.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor2.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor2 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor2-driver.out 2>&1
