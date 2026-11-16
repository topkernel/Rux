#!/bin/bash
# hg-bootw.sh (heal-gate lane): hg-boot.sh + debug=wwcensus — adds the
# PREEMPT-LEAK reporter rows (rip= + pctick= anatomy of heal candidates)
# so the healed carriers' wait sites can be named. Face slightly heavier
# on serial; heal/refusal prints identical.
set -uo pipefail
ROOT="/home/william/rux-agents/heal-gate"
cd "$ROOT"
TAG="${TAG:-hgw}"
SECS="${SECS:-540}"
KERNEL="${KERNEL:-$ROOT/target/x86_64-unknown-none/release/bzImage}"
DFX="txnhunt,fuxstuck,periodic,parkdump,schedtrace,wwcensus"
UD_SRC="/home/william/rux-agents/runq-strand/run/userdata-rsbase.img"
UD="$ROOT/run/userdata-$TAG.img"
AVAIL=$(LC_ALL=C free -m | awk '/^Mem:/{print $7}')
echo "=== $TAG start $(date +%H:%M:%S) avail=${AVAIL}M kernel=$KERNEL ==="
if [ "$AVAIL" -lt 10240 ]; then
  echo "avail mem ${AVAIL}M < 10G — waiting"
  while [ "$(LC_ALL=C free -m | awk '/^Mem:/{print $7}')" -lt 10240 ]; do sleep 60; done
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
for pat in 'KERNPANIC|kernel panic' 'RUNQ-STRAND tid=' 'MONOPOLIST nrrun' 'PREEMPT-HEAL cpu=' 'PREEMPT-HEAL-FIELD' 'PREEMPT-HEAL-REFUSED' 'PREEMPT-UNDERFLOW' 'PREEMPT-LEAK cpu=' 'WAKE-HEAL'; do
  printf '  %-22s %s\n' "$pat:" "$(grep -aEc "$pat" $S 2>/dev/null)"
done
echo "=== FACE DONE ($TAG) $(date +%H:%M:%S) ==="
