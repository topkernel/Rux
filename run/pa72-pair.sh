#!/bin/bash
# pa72-pair.sh — leg 46: socket flow-control discriminators on merged
# main 129a41fa (run/pa72-main-strong-bzImage, strong-profile x86_64).
# Two boots, same kernel, same image recipe:
#   pa72b = full pbench board WITH the new rows (p5e spin-drain child,
#           p5f MSG_DONTWAIT parent) — the discriminator medians
#   pa72c = longp5 mode + dfx=wwcensus on the cmdline — 12 s p5c2 /
#           p5c3 windows with serial markers; the WW heartbeat lines
#           (wakes/picks/avg_run/stranded) between the markers are the
#           census measurement
# Question (ledger 8.10.14 standing): decompose the 40-44 us
# cross-process class (read-draining child) vs the 4.8-5.4 us class
# (recvfrom-draining child) into wake/flow-control mechanism; name the
# lever if one exists. Honest bar: leg-43's wrapper attribution was
# withdrawn on evidence.
# Discipline: <=3 lane QEMUs, own PIDs only, host floor >=10G.
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa72-pair.log
K=run/pa72-main-strong-bzImage

stamp() { date '+%F %T'; }
l1() { awk '{printf "%d", $1}' /proc/loadavg; }
lane_q() {
  local n=0
  for pid in $(pgrep qemu-system-x86_64 2>/dev/null); do
    if tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null | grep -q 'rux-agents/perf-audit'; then
      n=$((n+1))
    fi
  done
  echo "$n"
}
memfloor() { awk '/MemAvailable/{printf "%d", $2/1048576}' /proc/meminfo; }

wait_slot() {
  while [ "$(lane_q)" -ge 2 ] || [ "$(memfloor)" -lt 10 ]; do
    echo "$(stamp) wait: lane_q=$(lane_q) mem_avail=$(memfloor)G load1=$(l1)" >> "$LOG"
    sleep 60
  done
}

echo "=== pa72 discriminator pair armed $(stamp) k=$K ===" >> "$LOG"

wait_slot
echo "--- boot pa72b (board + p5e/p5f) $(stamp) load1=$(l1) lane_q=$(lane_q) ---" >> "$LOG"
python3 test/pbench_boot.py --bzimage "$K" --tag pa72b \
    --accel tcg,thread=multi --smp 2 --mem 2G --timeout 1200 \
    > run/pa72b-boot.log 2>&1 &
B1=$!

wait_slot
echo "--- boot pa72c (longp5 + dfx=wwcensus) $(stamp) load1=$(l1) lane_q=$(lane_q) ---" >> "$LOG"
python3 test/pbench_boot.py --bzimage "$K" --tag pa72c \
    --accel tcg,thread=multi --smp 2 --mem 2G --timeout 900 \
    --long-p5 --append-extra "dfx=wwcensus" \
    > run/pa72c-boot.log 2>&1 &
B2=$!

wait $B1; echo "pa72b rc=$? ($(stamp))" >> "$LOG"
wait $B2; echo "pa72c rc=$? ($(stamp))" >> "$LOG"
echo "=== pa72 discriminator pair complete $(stamp) ===" >> "$LOG"
