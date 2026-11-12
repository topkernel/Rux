#!/bin/bash
# razor8c: pipe phase probe v3 (strict signature keying) — the full
# read-out phase sequence across the 8MB `yes | head` stream.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor8c.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor8c 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor8c-driver.out 2>&1
