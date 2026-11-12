#!/bin/bash
# fvset2 (109th): SETTLING boot on the fvpop2-populated base (R108
# kernel) — first-boot install churn finishes before the acceptance
# boots; the crash script runs (pat.bin written once).
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-fvset2.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py fvset2 420 run/system-fiF.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/fvset2-driver.out 2>&1
