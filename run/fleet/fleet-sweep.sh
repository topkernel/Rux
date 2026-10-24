#!/bin/bash
# fleet-sweep.sh — stress-fleet central sweep: status of every member,
# re-classification of any round flagged CRITICAL/WARN/WEDGE, and a
# fleet-wide summary. Run by hand or from a cron-ish loop:
#   while :; do fleet-sweep.sh; sleep 1800; done   (fleet-sweeprd.sh)
set -u
FLEET="$(cd "$(dirname "$0")" && pwd)"
LOGS="$FLEET/logs"
SUMMARY="$FLEET/sweep-latest.txt"

{
echo "================ stress-fleet sweep $(date '+%F %T') ================"
echo "--- host ---"
free -g | head -2
uptime

echo "--- members (driver procs) ---"
# union of members with rounds logs and members with live drivers (a
# member in its first round has no rounds.log yet)
for m in $( (ls "$LOGS" 2>/dev/null | grep -E '\-rounds\.log$' | sed 's/-rounds.log//'; pgrep -f 'fleet-member.sh' | xargs -r -I{} ps -o args= -p {} 2>/dev/null | awk '{print $3}' | sort -u) | sort -u); do
    alive=no
    pgrep -f "fleet-member.sh $m " >/dev/null 2>&1 && alive=yes
    last=$(tail -1 "$LOGS/$m-rounds.log" 2>/dev/null | cut -c1-160)
    rounds=$(wc -l < "$LOGS/$m-rounds.log" 2>/dev/null)
    echo "$m alive=$alive rounds=${rounds:-0} last: $last"
done

echo "--- adopted chains (frozen-kernel wildwrite capture) ---"
for c in ww-s2 wwload; do
    if pgrep -f "ww-chain2.sh $c|ww-loadchain.sh $c" >/dev/null 2>&1; then
        cur=$(ls -t "$FLEET"/../${c}-r*-serial.log 2>/dev/null | head -1)
        echo "$c ALIVE current: ${cur##*/}"
    else
        echo "$c DEAD — needs relaunch (see hunt ledger resume notes)"
    fi
done

echo "--- flagged rounds (last 50 across fleet) ---"
grep -hE " (CRITICAL|WARN|WEDGE) " "$LOGS"/*-rounds.log 2>/dev/null | tail -50
n_flagged=$(grep -hcE " (CRITICAL|WARN|WEDGE) " "$LOGS"/*-rounds.log 2>/dev/null | paste -sd+ | bc 2>/dev/null)
n_total=$(cat "$LOGS"/*-rounds.log 2>/dev/null | wc -l)
echo "flagged: ${n_flagged:-0}/$n_total rounds"

echo "--- qemu inventory (mine) ---"
pgrep -af "qemu-system" | grep -cE "fleet-|ww-hunt-frozen" | xargs echo "fleet-owned qemu count:"
} | tee "$SUMMARY"
