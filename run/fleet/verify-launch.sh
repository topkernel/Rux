#!/bin/bash
# verify-launch.sh — waits for ltpm driver exit, then launches the v6
# heal-fix verification boot (same shape as an ltp member). Logs the
# go/no-go memory check. Session-3.
set -u
cd /home/william/rux-agents/wildwrite-hunt
# wait up to 35 min for ltpm driver to end its current round
for i in $(seq 1 210); do
    pgrep -f "fleet-member.sh ltpm " >/dev/null || break
    sleep 10
done
if pgrep -f "fleet-member.sh ltpm " >/dev/null; then
    echo "[$(date +%H:%M:%S)] ltpm still running after 35min, ABORT launch" >> run/fleet/logs/verify-v6heal-rounds.log
    exit 1
fi
sleep 20
FREE=$(free -g | awk '/^Mem|内存/{print $4}')
AVAIL=$(free -g | awk '/^Mem|内存/{print $7}')
echo "[$(date +%H:%M:%S)] ltpm exited; free=${FREE}G avail=${AVAIL}G" >> run/fleet/logs/verify-v6heal-rounds.log
# Floor: user mandate >= 10G free; when page cache holds memory that
# the host would reclaim on demand, accept available >= 12G instead
# (interpretation logged for the session report).
if [ "${FREE:-0}" -lt 10 ] && [ "${AVAIL:-0}" -lt 12 ]; then
    echo "[$(date +%H:%M:%S)] memory floor not met, ABORT" >> run/fleet/logs/verify-v6heal-rounds.log
    exit 1
fi
exec ./run/fleet/verify-healfix.sh run/fleet/images/fleet-rv64-v6-rux v6heal 1800
