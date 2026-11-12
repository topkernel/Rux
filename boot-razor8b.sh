#!/bin/bash
# razor8b: pipe phase probes live — FI-PIPE write-in/read-out continuity
# names the corrupting side of the `yes | head` stream; also acceptance
# candidate #1 on the fully-fixed kernel.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor8b.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor8b 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor8b-driver.out 2>&1
