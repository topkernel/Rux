#!/bin/bash
# fvset: SETTLING boot — run the fvpop-populated base one more time so
# the first-boot install churn finishes BEFORE the crash-window
# acceptance boots (the fiE1-era pristine was long-settled; its sync(2)
# returned in ~22s. fvpop's base still churned during fv1: sync took
# 261s under a 24-EIO storm and the crashed image carried the acc1-era
# shifted-run damage family).
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-fvset.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py fvset 420 run/system-fiF.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/fvset-driver.out 2>&1
