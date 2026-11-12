#!/bin/bash
# fvi1 (109th): metadata_csum interop verification boot — fresh
# population from the TRUE pristine with the R109 kernel (extra_isize
# stamping + bg_itable_unused maintenance on top of R108). All inodes
# on the crashed image will be kernel-created; e2fsck -n is the gate.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-fvi1.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py fvi1 420 run/system-fiF.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/fvi1-driver.out 2>&1
