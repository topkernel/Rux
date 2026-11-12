#!/bin/bash
# razor9f: CONTROL BOOT — the pre-change kernel (HEAD 5daeaef0 binary,
# exactly what razor8b/8c ran) on pristine userdata. If this corrupts
# (+4-phase chunks) while razor9d/9e (my kernel) stay clean at full
# saturation, the fix is inside my diff and bisect names it.
cd /home/william/rux-agents/fi-fix-verify
exec env USERDATA=run/userdata-razor9f.img NOSNAP=1 SMP=8 LOGLEVEL=4 \
  python3 run/lb-boot-out.py razor9f 420 run/system-fiE1.img \
  run/bzImage-pre102 " " \
  > run/razor9f-driver.out 2>&1
