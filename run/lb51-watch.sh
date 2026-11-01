#!/bin/bash
L=/home/william/rux-agents/oh-launcher/run/oh-lb51-serial.log
while true; do
  sleep 45
  {
    echo "== $(date +%H:%M:%S) $(wc -l < $L) lines, last: $(tail -1 $L | cut -c1-60)"
    grep -E 'insert SA:200\b|CreateNewUser|startUser in service|userId not exist|lockstatus_100|USER_UNLOCKED|SwitchToUser|launcher.ready|appspawn start com' $L | tail -6
    echo "-- net (non-ctxmgr):"
    grep -E 'AMS46-SENDFAIL|AMS46-DRS|AMS46-WERR|death notice|AMS46-ARM dup' $L | grep -v no-ctxmgr | tail -6
    echo "-- arms (last):"
    grep -E 'AMS46-ARM pid' $L | tail -3
  } > /home/william/rux-agents/oh-launcher/run/lb51-watch.status 2>/dev/null
  pgrep -f 'lb-boot.py lb51' >/dev/null || { echo DONE >> /home/william/rux-agents/oh-launcher/run/lb51-watch.status; exit 0; }
done
