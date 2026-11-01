#!/bin/bash
# tail lb50 serial for the death-event net + account-phase chain
L=/home/william/rux-agents/oh-launcher/run/oh-lb50-serial.log
while true; do
  sleep 30
  {
    echo "== $(date +%H:%M:%S) $(wc -l < $L) lines, last: $(tail -1 $L | cut -c1-60)"
    grep -E 'insert SA:200\b|CreateNewUser|startUser in service|userId not exist|lockstatus_100|USER_UNLOCKED|SwitchToUser|com.ohos.launcher|AMS46-R txn.*code=0x46' $L | tail -8
    echo "-- net:"
    grep -E 'AMS46-SENDFAIL|AMS46-DRS|AMS46-WERR|death notice|AMS46-ARM' $L | grep -v no-ctxmgr | tail -8
  } > /home/william/rux-agents/oh-launcher/run/lb50-watch.status 2>/dev/null
  pgrep -f 'lb-boot.py lb50' >/dev/null || { echo DONE >> /home/william/rux-agents/oh-launcher/run/lb50-watch.status; exit 0; }
done
