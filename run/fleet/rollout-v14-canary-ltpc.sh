#!/bin/bash
# rollout-v14-canary-ltpc.sh — session 8: the v11 canary on ltp-c has
# answered (wedge survives the COW fix: r52 wedged at 643, r53 at 657
# vs one clean 845 crossing). Repurpose ltp-c as the v14-rv64 canary:
# v14 = main 94c670da + KILLDUMP stack-freed fix + BOTH dirty-page
# recycler fixes. The LTP surface is swap-active (r51 OOM census), so
# this canary also exercises the swap-out pending-order fix.
set -u
cd "$(dirname "$0")/../.." || exit 1
LOG=run/fleet/logs/rollout-v14-canary-ltpc.log
V14=$PWD/run/fleet/images/fleet-rv64-v14-rux
[ -f "$V14" ] || { echo "v14 rv64 image missing" >> "$LOG"; exit 1; }
wait_floor() {
    for i in $(seq 1 60); do
        [ "$(free -g | awk 'NR==2{print $7}')" -ge 12 ] && return 0
        sleep 30
    done
    return 1
}
echo "[$(date '+%F %T')] v14 canary start (ltp-c, from v11)" >> "$LOG"
touch run/fleet/ltp-c.stop
for i in $(seq 1 200); do
    pgrep -f "fleet-member.sh ltp-c[ ]" >/dev/null || break
    sleep 10
done
if pgrep -f "fleet-member.sh ltp-c[ ]" >/dev/null; then
    echo "[$(date +%H:%M:%S)] ltp-c did not exit in time; aborting" >> "$LOG"
    rm -f run/fleet/ltp-c.stop
    exit 1
fi
rm -f run/fleet/ltp-c.stop
wait_floor || { echo "[$(date +%H:%M:%S)] floor not clear; aborting" >> "$LOG"; exit 1; }
KIMG=$V14 ./run/fleet/stress-fleet.sh start ltp-c >> "$LOG" 2>&1
echo "[$(date +%H:%M:%S)] ltp-c relaunched on v14-rv64 (canary live)" >> "$LOG"
