#!/bin/bash
# s20 fsfuzz roll: swap the member onto the c5640ccc kernel (dfx-align
# tip c12e0221, md5 5afcb19c) at the r292 round boundary, for the
# DEADLOCK-family closure round. Detached by the caller; self-running.
W=/home/william/rux-agents/wwh-main-kernel
OLD_PID=478208   # fsfuzz driver bash (started 07:14 by s16-era session)
IMG=$W/run/fleet/images/fleet-rv64-c12e0221-rux

[ -f "$IMG" ] || { echo "missing image $IMG"; exit 1; }
touch "$W/run/fleet/fsfuzz.stop"
echo "$(date +%H:%M:%S) stop file set; waiting for driver $OLD_PID to end the current round"

for i in $(seq 1 270); do   # up to 45 min
    kill -0 "$OLD_PID" 2>/dev/null || break
    sleep 10
done
if kill -0 "$OLD_PID" 2>/dev/null; then
    echo "driver still alive after 45 min, ABORT (stop file left in place)"
    exit 1
fi
sleep 5
cd "$W" || exit 1
rm -f "$W/run/fleet/fsfuzz.stop"   # the new driver also clears it; belt and braces
KIMG="$IMG" setsid nohup bash run/fleet/fleet-member.sh fsfuzz fsfuzz 100000 293 \
    >> run/fleet/logs/fsfuzz-driver.log 2>&1 &
echo "$(date +%H:%M:%S) relaunched fsfuzz driver on c12e0221, first round 293"
