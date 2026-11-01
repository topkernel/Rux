#!/bin/bash
# Watch lb33 for the AMS46 ripple + thread snapshots.
LOG=run/oh-lb33-serial.log
while true; do
  if grep -q "AMS46 READ" "$LOG" 2>/dev/null; then
    echo "READ-SEEN $(date +%H:%M:%S)" >> run/lb33-watch.status
    sleep 240
    grep -n "AMS46" "$LOG" | grep -v "census" | tail -60 > run/lb33-ripple.txt
    grep -o "AMS46-T.*" "$LOG" | tail -12 > run/lb33-snaps.txt
    echo "CAPTURED $(date +%H:%M:%S)" >> run/lb33-watch.status
    exit 0
  fi
  if ! pgrep -f "lb-boot.py lb33" > /dev/null 2>&1; then
    echo "BOOT-ENDED-NO-READ $(date +%H:%M:%S)" >> run/lb33-watch.status
    exit 1
  fi
  sleep 10
done
