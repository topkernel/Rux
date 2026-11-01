#!/bin/bash
# lb-txnstats.sh — SLOW-TXN distribution + ladder-event summary from an lb serial log.
# Usage: lb-txnstats.sh run/oh-lbNN-serial.log
LOG="$1"
echo "== SLOW-TXN exec distribution (ms) =="
grep -o "SLOW-TXN txn[0-9]* code [0-9]* from pid [0-9]* tid [0-9]* ([^)]*) to pid [0-9]* tid [0-9]* ([^)]*): total [0-9]* ms (queue [0-9]* ms, exec [0-9]*" "$LOG" \
 | sed 's/SLOW-TXN txn\([0-9]*\) code \([0-9]*\) from pid [0-9]* tid [0-9]* (\([^)]*\)) to pid [0-9]* tid [0-9]* (\([^)]*\)): total \([0-9]*\) ms (queue \([0-9]*\) ms, exec \([0-9]*\)/\2 \3 \4 \6 \7/' \
 | sort -k5 -n > /tmp/txn.$$
total=$(wc -l < /tmp/txn.$$)
if [ "$total" -gt 0 ]; then
  awk '{print $5}' /tmp/txn.$$ | sort -n | awk '{a[NR]=$1} END {printf "count=%d min=%d p50=%d p90=%d max=%d\n", NR, a[1], a[int((NR+1)*0.5)], a[int((NR+1)*0.9)], a[NR]}'
  echo "-- worst 10 (code from -> to, queue_ms exec_ms):"
  sort -k5 -n /tmp/txn.$$ | tail -10 | awk '{printf "  code=%s %s -> %s q=%s exec=%s\n", $1, $2, $3, $4, $5}'
else
  echo "count=0"
fi
echo "== watchdog kills =="
echo "KOT-KILL lines: $(grep -c 'KOT-KILL' "$LOG" 2>/dev/null)"
echo "== ladder events =="
grep -E "insert SA:200|CreateNewUser|startUser in service|SwitchToUser|foregroundUserId|userId not exist|OnStartUserDone|AbilityInfo|com.ohos.launcher|lockstatus_100|USER_UNLOCKED|USER_ADDED" "$LOG" 2>/dev/null | grep -v "not set" | head -40
echo "== accountmgr lifecycle =="
grep -E "accountmgr" "$LOG" 2>/dev/null | grep -E "insert SA|KOT-KILL|ServiceExec|signal|exit|died|Restart" | head -10
