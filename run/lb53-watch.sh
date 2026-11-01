#!/bin/bash
L=/home/william/rux-agents/oh-launcher/run/oh-lb53-serial.log
while true; do
  sleep 40
  {
    echo "== $(date +%H:%M:%S) lines=$(wc -l < $L) inserts=$(grep -c 'insert SA' $L) dl=$(grep -c DEADLOCK $L) last: $(tail -1 $L | cut -c1-50)"
    echo "-- chain:"
    grep -E 'insert SA:200\b|CreateNewUser|startUser in service|userId not exist|lockstatus_100|USER_UNLOCKED|launcher.ready|AMS46 ENQ|SwitchToUser|foregroundUserId' $L | tail -6 | cut -c1-115
    echo "-- net:"
    grep -E 'AMS46-SENDFAIL|AMS46-DRS|AMS46-WERR|death notice|ARM dup|failed to get os account|fail, result:' $L | grep -v no-ctxmgr | tail -6 | cut -c1-115
  } > /home/william/rux-agents/oh-launcher/run/lb53-watch.status 2>/dev/null
  pgrep -f 'lb-boot.py lb53' >/dev/null || { echo DONE >> /home/william/rux-agents/oh-launcher/run/lb53-watch.status; exit 0; }
done
