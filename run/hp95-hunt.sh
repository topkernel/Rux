#!/bin/bash
# hp95 hunt series (95th): the clean-stall class (~2/4 boots reach no
# launcher with 0 guards / 0 aborts). Arm = leakhunt (binder rings, proven
# not to suppress the stall) + periodic (30s whole-task snapshots from the
# timer softirq — on a stalled boot the dump names every task's parked
# syscall, the wedge-namer for this class). Rig identical to hp94 series:
# pristine userdata-fresh, SMP8+4G KVM, LOGLEVEL=4, 330s.
cd /home/william/rux-agents/oh-crash
K=target/x86_64-unknown-none/debug/bzImage
for i in e f g h; do
  tag="hp95${i}"
  echo "=== $tag start $(date +%H:%M:%S) ==="
  USERDATA=run/userdata-fresh.img SMP=8 LOGLEVEL=4 \
    python3 run/lb-boot-out.py "$tag" 330 run/oh-img/system.img "$K" "dfx=leakhunt,periodic" \
    > "run/${tag}-driver.out" 2>&1
  sleep 10
  g=$(grep -ac "SYNCSEND-GUARD" "run/oh-${tag}-serial.log" 2>/dev/null || echo 0)
  a=$(grep -ac "signal 6" "run/oh-${tag}-serial.log" 2>/dev/null || echo 0)
  l=$(grep -ac "bootevent.launcher.ready" "run/oh-${tag}-markers.txt" 2>/dev/null || echo 0)
  an=$(grep -ac "bootevent.bootanimation.finished" "run/oh-${tag}-markers.txt" 2>/dev/null || echo 0)
  d=$(grep -ac "DFX TASK DUMP" "run/oh-${tag}-serial.log" 2>/dev/null || echo 0)
  echo "=== $tag guards=${g:-0} aborts=${a:-0} animfin=${an:-0} launcher_ready=${l:-0} taskdumps=${d:-0} $(date +%H:%M:%S) ==="
done
echo "=== hunt done $(date +%H:%M:%S) ==="
