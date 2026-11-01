#!/bin/bash
LOG=run/oh-lb36-serial.log
S=run/lb36-watch.status
while true; do
  grep -q "ServiceStart starting:installs" "$LOG" 2>/dev/null && { echo "INSTALLS-STARTED $(grep -m1 ServiceStart.*starting:installs $LOG | cut -c1-14)" >> $S; }
  grep -q "AMS46 READ" "$LOG" 2>/dev/null && { echo "READ-SEEN $(date +%H:%M:%S)" >> $S; }
  grep -q "com.ohos.launcher" "$LOG" 2>/dev/null && { echo "LAUNCHER-MENTION $(date +%H:%M:%S)" >> $S; }
  grep -q "launcher.ready=true\|bootevent.launcher.ready true" "$LOG" 2>/dev/null && { echo "LAUNCHER-READY $(date +%H:%M:%S)" >> $S; }
  tail -1 $S | grep -q CAPTURED && exit 0
  if ! pgrep -f "lb-boot.py lb36" > /dev/null 2>&1; then
    echo "BOOT-ENDED $(date +%H:%M:%S)" >> $S; exit 1
  fi
  # capture 300s after the READ
  if grep -q "AMS46 READ" "$LOG" 2>/dev/null; then
    R=$(date +%s)
    while [ $(( $(date +%s) - R )) -lt 300 ]; do sleep 15; done
    grep -n "AMS46-T\|HILOG-AMS\|AMS46 ENQ\|AMS46 READ\|AMS46 FREE\|AMS46 census\|installs\|launcher\|AMSI1640\|appspawn start\|SwitchToUser" "$LOG" 2>/dev/null | tail -150 > run/lb36-ams.txt
    echo "CAPTURED $(date +%H:%M:%S)" >> $S
    exit 0
  fi
  sleep 10
done
