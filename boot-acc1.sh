#!/bin/bash
# acc1: acceptance boot #1 on the fixed kernel (Ok(0) reorder + EIO retry).
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-acc1.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py acc1 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/acc1-driver.out 2>&1
