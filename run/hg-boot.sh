#!/bin/bash
# hg-boot.sh (heal-gate lane, 122nd): one rs2-class boot of the OH
# wedge-repro userdata base with the HEAL-GATE kernel — the same face
# as runq-strand's rs-boot.sh (KVM q35 SMP8 8G, NOSNAP userdata,
# dfx=txnhunt,fuxstuck,periodic,parkdump,schedtrace), but each boot
# gets its own PRIVATE full copy of the rsbase userdata so the shared
# base is never mutated (ext4: no reflink, 16G per boot — deleted at
# script end; keep only one live at a time).
# Usage: TAG=hg1 KERNEL=<bzImage> SECS=540 bash run/hg-boot.sh
set -uo pipefail
ROOT="/home/william/rux-agents/heal-gate"
cd "$ROOT"
TAG="${TAG:-hg}"
SECS="${SECS:-540}"
KERNEL="${KERNEL:-$ROOT/target/x86_64-unknown-none/release/bzImage}"
DFX="txnhunt,fuxstuck,periodic,parkdump,schedtrace"
UD_SRC="/home/william/rux-agents/runq-strand/run/userdata-rsbase.img"
UD="$ROOT/run/userdata-$TAG.img"

AVAIL=$(LC_ALL=C free -m | awk '/^Mem:/{print $7}')
echo "=== $TAG start $(date +%H:%M:%S) avail=${AVAIL}M kernel=$KERNEL ==="
if [ "$AVAIL" -lt 10240 ]; then
  echo "avail mem ${AVAIL}M < 10G — waiting"
  while [ "$(LC_ALL=C free -m | awk '/^Mem:/{print $7}')" -lt 10240 ]; do
    sleep 60
  done
fi

if [ ! -f "$UD" ]; then
  echo "copying userdata base (16G, full copy on ext4)..."
  cp --reflink=auto "$UD_SRC" "$UD" || { echo "userdata copy failed"; exit 1; }
fi

export NOSNAP=1
export USERDATA="$UD"
SMP=8 MEM=8G LOGLEVEL=4 python3 run/lb-boot-qmp.py "$TAG" "$SECS" \
  "run/oh-img/system.img" "$KERNEL" \
  "dfx=$DFX trace_event=input" \
  > "run/${TAG}-driver.out" 2>&1
rc=$?
rm -f "$UD"
S="run/oh-${TAG}-serial.log"
echo "=== $TAG rc=$rc $(date +%H:%M:%S) ==="
echo "  panics:            $(grep -ac 'KERNPANIC\|kernel panic' $S 2>/dev/null)"
echo "  RUNQ-STRAND rows:  $(grep -ac 'RUNQ-STRAND tid=' $S 2>/dev/null)"
echo "  MONOPOLIST rows:   $(grep -ac 'MONOPOLIST nrrun' $S 2>/dev/null)"
echo "  PREEMPT-HEAL rows: $(grep -ac 'PREEMPT-HEAL cpu=' $S 2>/dev/null)"
echo "  PREEMPT-HEAL-FIELD:$(grep -ac 'PREEMPT-HEAL-FIELD' $S 2>/dev/null)"
echo "  HEAL-REFUSED rows: $(grep -ac 'PREEMPT-HEAL-REFUSED' $S 2>/dev/null)"
echo "  PREEMPT-UNDERFLOW: $(grep -ac 'PREEMPT-UNDERFLOW' $S 2>/dev/null)"
echo "  FUX-STUCK lines:   $(grep -ac 'FUX-STUCK' $S 2>/dev/null)"
echo "  WAKE-HEAL rows:    $(grep -ac 'WAKE-HEAL' $S 2>/dev/null)"
echo "=== FACE DONE ($TAG) $(date +%H:%M:%S) ==="
