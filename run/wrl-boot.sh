#!/bin/bash
# wrl-boot.sh (wrl-class lane): the launcher-race f2 recipe on CURRENT
# main — WAKE-RUN-LEAK census anatomy + the voluntary-exit-owing-txn
# verdict boots. Same rig as neverword nw-boot.sh (KVM q35 SMP8/8G,
# private 16G copy of pristine userdata-fresh3, NOSNAP first-boot
# population) but WITHOUT parkdump (the RWFRZ/park stack flood that
# interleaved the lr-series serials) and WITH bindertrace (adds
# THREAD-DIED-SERVICING: names every thread that dies with a non-empty
# txn stack or todo — the item-2 discriminator).
# Usage: TAG=w1 [SECS=540] bash run/wrl-boot.sh
set -uo pipefail
cd /home/william/rux-agents/wrl-class
TAG="${TAG:-w}"
SECS="${SECS:-540}"
KERNEL="${KERNEL:-$PWD/target/x86_64-unknown-none/release/bzImage}"
PRISTINE="${PRISTINE:-run/oh-img/userdata-fresh3.img}"
EXTRA="dfx=pctlmon,fuxstuck,txnhunt,bindertrace,periodic trace_event=input"
UD="run/ud-${TAG}.img"

AVAIL=$(LC_ALL=C free -m | awk '/^Mem:/{print $7}')
echo "=== $TAG start $(date +%H:%M:%S) avail=${AVAIL}M kernel=$KERNEL secs=$SECS extra='$EXTRA' load='$(cat /proc/loadavg)' ==="
[ "$AVAIL" -lt 10240 ] && { echo "avail mem ${AVAIL}MB < 10G — waiting"; while [ "$(LC_ALL=C free -m | awk '/^Mem:/{print $7}')" -lt 10240 ]; do sleep 60; done; }

if [ ! -f "$UD" ]; then
  echo "copying userdata base (16G full copy)..."
  cp --reflink=auto "$PRISTINE" "$UD" || { echo "userdata copy failed"; exit 1; }
fi

USERDATA="$UD" SMP=8 MEM=8G LOGLEVEL=4 NOSNAP=1 \
  python3 run/lb-boot-qmp.py "$TAG" "$SECS" run/oh-img/system.img "$KERNEL" "$EXTRA" \
  > "run/${TAG}-driver.out" 2>&1
rc=$?
echo "=== $TAG rc=$rc $(date +%H:%M:%S) load='$(cat /proc/loadavg)' ==="
S="run/oh-${TAG}-serial.log"
echo "  panics:            $(grep -ac 'KERNPANIC\|kernel panic' "$S" 2>/dev/null)"
echo "  ExecReboot:        $(grep -ac 'ExecReboot' "$S" 2>/dev/null)"
echo "  WAKE-RUN-LEAK:     $(grep -ac 'WAKE-RUN-LEAK tid' "$S" 2>/dev/null)"
echo "  THREAD-DIED-SERV:  $(grep -ac 'THREAD-DIED-SERVICING' "$S" 2>/dev/null)"
echo "  UIEXIT rows:       $(grep -ac 'UIEXIT' "$S" 2>/dev/null)"
echo "  UIEXIT owing>0:    $(grep -a 'UIEXIT' "$S" 2>/dev/null | grep -ac 'owing_n=[1-9]')"
echo "  RERR-PUSH:         $(grep -ac 'RERR-PUSH' "$S" 2>/dev/null)"
echo "  TXN-HANG-OWE:      $(grep -ac 'TXN-HANG-OWE' "$S" 2>/dev/null)"
echo "  AMS46-DRS:         $(grep -ac 'AMS46-DRS' "$S" 2>/dev/null)"
cat "run/oh-${TAG}-markers.txt" 2>/dev/null | sed 's/^/    MK /'
echo "=== WRL BOOT DONE ($TAG) $(date +%H:%M:%S) ==="
