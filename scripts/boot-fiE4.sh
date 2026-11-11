#!/bin/bash
# fiE4: crash-window rig (data-integrity lane) — the guest writes 8 MB,
# calls sync(2) and PARKS; this driver terminates QEMU after SECS seconds:
# a power cut from the guest's point of view (no guest-side shutdown flush).
#
# The guest script lives in the system image (run/system-fiE1.img, a copy
# of run/system-fi4.img with run/fi-inspect/hp87-run-fiE1.sh injected at
# /system/etc/hp87-run.sh).
#
#   bash scripts/boot-fiE4.sh
#
# After the boot: scripts/fi_inspect.py run/userdata-fiE4.img /local/tmp/pat.bin
# must print size=8388608 and md5=01cabbdd75d72d410fa93a5942ac4247.
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"
exec env USERDATA=run/userdata-fiE4.img NOSNAP=1 SMP=8 python3 run/lb-boot-out.py fiE4 90 run/system-fiE1.img target/x86_64-unknown-none/debug/bzImage " " > run/fiE4-driver.out 2>&1
