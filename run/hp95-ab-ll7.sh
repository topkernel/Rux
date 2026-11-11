#!/bin/bash
# hp95 hot-lever A/B (95th, §94.5#4 lever a): LOGLEVEL=7 vs LOGLEVEL=4
# same-rig A/B on the storm-fixed kernel. The 94.3 tail table named the
# flat-tail burn hilogd 32.9k + systemui 30.6k + settingsdata 29.0k kernel
# samples; LOGLEVEL gates how much kmsg the kernel prints (and bills to
# whoever is current). Rig: pristine userdata-fresh, SMP8+4G KVM,
# dfx=leakhunt,periodic (same arm as the hunt), 330s. LOGLEVEL=4 arm =
# the hunt series itself (hp95e/f/g/h). This script runs the LOGLEVEL=7
# arm (kernel default stays 7; the rig scripts pass the level).
cd /home/william/rux-agents/oh-crash
# wait for the hunt series to release the rig
while ! grep -q "hunt done" run/hp95-hunt.log 2>/dev/null; do sleep 30; done
K=target/x86_64-unknown-none/debug/bzImage
for i in i j; do
  tag="hp95${i}"
  echo "=== $tag (LOGLEVEL=7) start $(date +%H:%M:%S) ==="
  USERDATA=run/userdata-fresh.img SMP=8 LOGLEVEL=7 \
    python3 run/lb-boot-out.py "$tag" 330 run/oh-img/system.img "$K" "dfx=leakhunt,periodic" \
    > "run/${tag}-driver.out" 2>&1
  sleep 10
  g=$(grep -ac "SYNCSEND-GUARD" "run/oh-${tag}-serial.log" 2>/dev/null || echo 0)
  a=$(grep -ac "signal 6" "run/oh-${tag}-serial.log" 2>/dev/null || echo 0)
  l=$(grep -a "launcher.ready" "run/oh-${tag}-markers.txt" 2>/dev/null | awk '{print $1}')
  echo "=== $tag guards=${g:-0} aborts=${a:-0} launcher_ready=${l:-none} $(date +%H:%M:%S) ==="
done
echo "=== ab-ll7 done $(date +%H:%M:%S) ==="
