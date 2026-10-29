#!/bin/bash
# gdm-evidence-watch.sh — rerun the gdm evidence boot (r17: SWI core
# attribution of the SIGKILL storm) when the host load allows a sane
# TCG pace. Threshold <5 foreign QEMUs (looser than the perf gate's
# <2: evidence boots do not need a quiet host, only a usable one), and
# yields to any running perf-gate attempt. Log: run/gdm-evidence.log.
cd "$(dirname "$0")/.." || exit 1
LOG=run/gdm-evidence.log
echo "watcher armed $(date)" > "$LOG"
tries=0
while [ "$tries" -lt 96 ]; do           # 8 h at 300 s
  tries=$((tries + 1))
  n=$(pgrep -c qemu-system 2>/dev/null || true)
  n="${n:-0}"
  gate_busy=$(pgrep -fc 'perf-gate.sh' 2>/dev/null || true)
  if [ "$n" -lt 5 ] && [ "${gate_busy:-0}" -eq 0 ]; then
    echo "evidence window at $(date) (foreign=$n) — booting gdmr17b" >> "$LOG"
    python3 test/ubuntu-gnome/fire.py --tag gdmr17b \
      --img work/gnome-gdm-r1.img --seconds 900 --dump-every 30 \
      --dfx watchdog,inputtrace \
      --extra-append 'runit-mode=gdm loglevel=7' \
      >> "$LOG" 2>&1
    echo "gdmr17b boot rc=$? at $(date)" >> "$LOG"
    grep -a -E 'SWI9|DEQUEUE sig=9' runs/gnome-gdmr17b/serial.log \
      >> "$LOG" 2>&1 | head -40
    exit 0
  fi
  sleep 300
done
echo "no evidence window in 8h; giving up at $(date)" >> "$LOG"
exit 3
