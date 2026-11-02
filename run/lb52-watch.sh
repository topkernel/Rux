#!/bin/bash
L=/home/william/rux-agents/oh-launcher/run/oh-lb52-serial.log
while true; do
  sleep 60
  {
    echo "== $(date +%H:%M:%S) $(wc -l < $L) lines, last: $(tail -1 $L | cut -c1-55)"
    echo "-- chain:"
    grep -E 'insert SA:200\b|CreateNewUser|startUser in service|userId not exist|lockstatus_100|USER_UNLOCKED|launcher.ready|AMS46 ENQ' $L | tail -5 | cut -c1-110
    echo "-- net:"
    grep -E 'AMS46-SENDFAIL|AMS46-DRS|AMS46-WERR|death notice|ARM dup' $L | grep -v no-ctxmgr | tail -5 | cut -c1-110
    echo "-- deadlock count: $(grep -c DEADLOCK $L)"
  } > /home/william/rux-agents/oh-launcher/run/lb52-watch.status 2>/dev/null
  pgrep -f 'lb-boot.py lb52' >/dev/null || { echo DONE >> /home/william/rux-agents/oh-launcher/run/lb52-watch.status; exit 0; }
done
