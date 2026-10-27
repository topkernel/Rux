#!/bin/bash
# pa61-route1.sh — leg-38 track-2: the F4 route-1 verification chain
# (route-1 = 22452d60 null-journal deferral; merge claim needs the
# fsw pair read, the LTP creat05/fork09-class spot run, and the
# mkdrmdir/unlink medians — the medians ride the pa60 pbench pair,
# this chain covers the other two). Functional steps: no quiet gate
# (state compare + volume counters are noise-immune), but waits for
# the pa60 census chain to release the QEMU slot. <=1 lane QEMU.
#   1. LTP spot: branch riscv64 debug kernel (run/pa60-ltp-branch-rux)
#      on run/pa59-ltp-r1-rootfs.img (17 metadata/create rows),
#      900 s, tag pa61ltp-r1. Compare vs fleet baseline rows.
#   2. fsw mech boot: branch HEAD x86 debug (run/pa60-fswm-bzImage)
#      fsw=1, 480 s, tag pa61fswm — WITH the FSW1 sd separator fix
#      (3750e17c), so sync_dirty/skip read unambiguously.
#   3. fsw ctl boot: PURE main b4da2106 x86 debug (pa52-main kernel)
#      fsw=1, 480 s, tag pa61fswc — the honest same-class control
#      (route-1 absent; sd stays concatenated there, pb=0 separates).
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa61-route1.log
LTPK=run/pa60-ltp-branch-rux
MECHK=run/pa60-fswm-bzImage
CTLK=/home/william/rux-agents/pa52-main/target/x86_64-unknown-none/debug/bzImage

echo "=== pa61-route1 armed $(date '+%F %T') (waits for pa60 chain; then LTP spot + fsw pair) ===" >> "$LOG"

# Stage 0: wait for the pa60 census chain (it owns the QEMU slot).
while pgrep -f "pa60-census.sh" > /dev/null 2>&1; do
  echo "$(date '+%F %T') waiting: pa60-census chain still running" >> "$LOG"
  sleep 180
done
echo "$(date '+%F %T') pa60 chain released the slot" >> "$LOG"

# Step 1: LTP spot run (functional).
if [ -f "$LTPK" ] && [ -f run/pa59-ltp-r1-rootfs.img ]; then
  echo "$(date '+%F %T') LTP spot boot pa61ltp-r1 start (kernel=$LTPK)" >> "$LOG"
  bash run/pa59-ltp-r1.sh "$LTPK" pa61ltp-r1 >> "$LOG" 2>&1
  # pa59-ltp-r1.sh backgrounds the QEMU behind its own `timeout 900s`
  # wrapper (self-terminating); poll for the sweep marker or let the
  # wrapper reap it. No kill needed (pkill is forbidden; not ours to
  # signal beyond the wrapper's own timeout).
  for i in $(seq 1 40); do
    sleep 30
    if grep -q "SWEEP DONE\|power-down\|reboot: " run/pa-pa61ltp-r1-serial.log 2>/dev/null; then break; fi
  done
  sleep 15
  echo "$(date '+%F %T') LTP spot boot finished (log run/pa-pa61ltp-r1-serial.log)" >> "$LOG"
else
  echo "$(date '+%F %T') LTP spot SKIPPED — kernel or rootfs missing" >> "$LOG"
fi

# Step 2+3: fsw mechanism + control pair (each 480 s, serialized).
for spec in "pa61fswm:$MECHK:mech" "pa61fswc:$CTLK:ctl"; do
  tag="${spec%%:*}"; rest="${spec#*:}"; k="${rest%%:*}"; which="${rest##*:}"
  if [ ! -f "$k" ]; then
    echo "$(date '+%F %T') fsw boot $which SKIPPED — kernel missing ($k)" >> "$LOG"
    continue
  fi
  echo "$(date '+%F %T') fsw boot $which tag=$tag start (kernel=$k)" >> "$LOG"
  APPEND_EXTRA="fsw=1" bash run/pa-boot.sh "$tag" "$k" 480 >> "$LOG" 2>&1
  rc=$?
  echo "$(date '+%F %T') fsw boot $which tag=$tag rc=$rc" >> "$LOG"
done
echo "=== pa61-route1 complete $(date '+%F %T') ===" >> "$LOG"
exit 0
