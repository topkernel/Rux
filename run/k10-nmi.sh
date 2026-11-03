#!/bin/bash
# k10-nmi.sh — wedge, then inject NMI via QEMU monitor; watch if machine resumes.
set -u
cd /home/william/rux-agents/ltp-r21
LOG="run/$1"
FIFO=run/k10.fifo
MON="run/$1.mon"
rm -f "$FIFO" "$LOG" "$MON"; mkfifo "$FIFO"
(
  sleep 12
  echo "/test/linux-ltp/testcases/bin/kill10 -d 4 -n 10 -g 2 > /tmp/k10.out 2>&1 </dev/null &" > "$FIFO"
  sleep 25
) &
FEEDER=$!
timeout 60 qemu-system-x86_64 -M q35 -accel kvm -cpu max -m 2G -smp 2 \
  -display none -serial stdio -monitor unix:"$MON",server,nowait -no-reboot -snapshot \
  -drive file=test/rootfs-x86_64-ltp.img,if=none,id=rootfs,format=raw,file.locking=off \
  -device virtio-blk-pci,drive=rootfs \
  -kernel target/x86_64-unknown-none/debug/bzImage \
  -append "root=/dev/vda rw init=/bin/sh console=ttyS0" \
  < "$FIFO" > "$LOG" 2>&1 &
QPID=$!
sleep 30
python3 run/monq.py "$MON" "info cpus" "nmi" > "${LOG%.log}-mon.txt" 2>&1
sleep 6
python3 run/monq.py "$MON" "info cpus" >> "${LOG%.log}-mon.txt" 2>&1
sleep 10
echo "== console tail after NMI ==" >> "${LOG%.log}-mon.txt"
tail -4 "$LOG" >> "${LOG%.log}-mon.txt"
kill -9 $QPID 2>/dev/null
kill $FEEDER 2>/dev/null; rm -f "$FIFO"
echo "NMI PROBE DONE"
