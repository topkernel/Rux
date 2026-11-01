#!/bin/bash
LOG=run/oh-lb34-serial.log
while true; do
  if grep -q "AMS46 READ" "$LOG" 2>/dev/null; then
    echo "READ-SEEN $(date +%H:%M:%S)" >> run/lb34-watch.status
    sleep 240
    grep -n "AMS46-T\|HILOG-AMS\|AMS46 ENQ\|AMS46 READ\|AMS46 FREE\|AMS46 RIPPLE\|AMS46 census" "$LOG" | tail -120 > run/lb34-ams.txt
    echo "CAPTURED $(date +%H:%M:%S) lines=$(wc -l < run/lb34-ams.txt)" >> run/lb34-watch.status
    exit 0
  fi
  if ! pgrep -f "lb-boot.py lb34" > /dev/null 2>&1; then
    echo "BOOT-ENDED-NO-READ $(date +%H:%M:%S)" >> run/lb34-watch.status
    exit 1
  fi
  sleep 10
done
