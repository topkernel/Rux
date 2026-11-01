#!/bin/bash
# Watch lb32 for the AMS46 ripple: wait for ENQ, then capture the window.
LOG=run/oh-lb32-serial.log
while true; do
  if grep -q "AMS46 ENQ" "$LOG" 2>/dev/null; then
    echo "ENQ-SEEN $(date +%H:%M:%S)" >> run/lb32-watch.status
    sleep 150
    grep -n "AMS46" "$LOG" | tail -80 > run/lb32-ripple.txt
    echo "RIPPLE-CAPTURED $(date +%H:%M:%S) lines=$(wc -l < run/lb32-ripple.txt)" >> run/lb32-watch.status
    exit 0
  fi
  if ! pgrep -f "lb-boot.py lb32" > /dev/null 2>&1; then
    echo "BOOT-ENDED-NO-ENQ $(date +%H:%M:%S)" >> run/lb32-watch.status
    exit 1
  fi
  sleep 10
done
