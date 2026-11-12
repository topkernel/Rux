#!/bin/bash
# fv5 (109th): R108 acceptance boot #2 — settled fvset2 base,
# crash-window rig (write pat.bin, sync, park, hard kill at 420s).
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-fv5.img NOSNAP=1 SMP=8 LOGLEVEL=4   python3 run/lb-boot-out.py fv5 420 run/system-fiF.img   target/x86_64-unknown-none/debug/bzImage " "   > run/fv5-driver.out 2>&1
