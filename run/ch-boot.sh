#!/bin/bash
# ch-boot.sh (carrier-heal lane, 123rd): one rs2/hg-class boot of the OH
# wedge-repro userdata base with the CARRIER-HEAL kernel — same rig as
# heal-gate's run/hg-boot.sh (KVM q35 SMP8 8G, NOSNAP userdata, private
# full copy of the rsbase userdata, deleted at script end), with
# wwcensus ON so the PREEMPT-LEAK census names live carrier sites and
# the REFUSED rows name the refusing evidence (via=A acquire / via=C
# softirq-window carrier).
# Usage: TAG=ch1 KERNEL=<bzImage> SECS=540 bash run/ch-boot.sh
set -uo pipefail
ROOT="/home/william/rux-agents/carrier-heal"
cd "$ROOT"
TAG="${TAG:-ch}"
SECS="${SECS:-540}"
KERNEL="${KERNEL:-$ROOT/target/x86_64-unknown-none/release/bzImage}"
DFX="txnhunt,fuxstuck,periodic,parkdump,schedtrace,wwcensus"
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
echo "  REFUSED via=C:     $(grep -ac 'via=C' $S 2>/dev/null)"
echo "  REFUSED via=A:     $(grep -ac 'via=A' $S 2>/dev/null)"
echo "  PREEMPT-UNDERFLOW: $(grep -ac 'PREEMPT-UNDERFLOW' $S 2>/dev/null)"
echo "  FUX-STUCK lines:   $(grep -ac 'FUX-STUCK' $S 2>/dev/null)"
echo "  WAKE-HEAL rows:    $(grep -ac 'WAKE-HEAL' $S 2>/dev/null)"
echo "  PARKED-HEAL rows:  $(grep -ac 'PARKED-HEAL' $S 2>/dev/null)"
echo "  TICK-GHOST rows:   $(grep -ac 'TICK-GHOST' $S 2>/dev/null)"
echo "  DEADLOCK rows:     $(grep -ac 'DEADLOCK' $S 2>/dev/null)"
echo "=== FACE DONE ($TAG) $(date +%H:%M:%S) ==="
