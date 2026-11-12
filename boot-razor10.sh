#!/bin/bash
# razor10: ACCEPTANCE CANDATE #2 — fixed kernel (virtio backpressure
# f702e98e + corrected probes) on the RESTORED pristine userdata
# (post-contamination fix). Target: size=8388608, md5=01cabbdd…4247.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor10.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor10 420 run/system-fiE1.img \
  target/x86_64-unknown-none/debug/bzImage " " \
  > run/razor10-driver.out 2>&1
