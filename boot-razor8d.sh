#!/bin/bash
# razor8d: pipe probe v4 — full-chunk periodicity (torn_at) + per-side
# running totals: between-chunk vs in-chunk loss, write-in vs read-out
# byte accounting.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor8d.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor8d 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor8d-driver.out 2>&1
