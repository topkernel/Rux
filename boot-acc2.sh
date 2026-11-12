#!/bin/bash
# acc2: acceptance boot #2 on the fixed kernel.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-acc2.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py acc2 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/acc2-driver.out 2>&1
