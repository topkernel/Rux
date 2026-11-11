#!/bin/bash
# hp94 series (94th): validate the single-slot error dedup fix (63dbeae4)
# against the SYNCSEND-GUARD storm. Rig identical to hp93g (leakhunt arm,
# pristine userdata-fresh, SMP8+4G, LOGLEVEL=4, 330s). Score per boot:
#   guard hits       (want 0; 92nd's storm: 25-80)
#   dedup drops      (proof the old dual-delivery engine FIRED and was caught)
#   dual ERR_RECV    (the poison shape itself, from hist rings)
#   foundation aborts (want 0)
cd /home/william/rux-agents/oh-crash
K=target/x86_64-unknown-none/debug/bzImage
for i in a b c d; do
  tag="hp94${i}"
  echo "=== $tag start $(date +%H:%M:%S) ==="
  USERDATA=run/userdata-fresh.img SMP=8 LOGLEVEL=4 \
    python3 run/lb-boot-out.py "$tag" 330 run/oh-img/system.img "$K" "dfx=leakhunt" \
    > "run/${tag}-driver.out" 2>&1
  sleep 10
  g=$(grep -ac "SYNCSEND-GUARD" "run/oh-${tag}-serial.log" 2>/dev/null || echo 0)
  d=$(grep -ac "Unexpected reply error" "run/oh-${tag}-serial.log" 2>/dev/null || echo 0)
  a=$(grep -ac "signal 6" "run/oh-${tag}-serial.log" 2>/dev/null || echo 0)
  l=$(grep -ac "bootevent.launcher.ready" "run/oh-${tag}-markers.txt" 2>/dev/null || echo 0)
  echo "=== $tag guards=${g:-0} dedup_drops=${d:-0} aborts=${a:-0} launcher_ready_marker=${l:-0} $(date +%H:%M:%S) ==="
done
echo "=== series done $(date +%H:%M:%S) ==="
