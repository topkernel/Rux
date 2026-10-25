#!/bin/bash
# rollout-v7.sh — gated fleet refresh onto the merged tree (session 3).
# Gate: ltp-a round 22 (already on v7) must classify WITHOUT kernel
# panics. Then: riscv members stop+relaunch on v7 (KIMG), default images
# swapped, monkey members restarted on the swapped x86 image.
set -u
cd /home/william/rux-agents/wildwrite-hunt
LOG=run/fleet/logs/rollout-v7.log
V7=$PWD/run/fleet/images/fleet-rv64-v7-merged-rux
echo "[$(date +%H:%M:%S)] rollout-v7 waiting for ltp-a R22 verdict" >> $LOG

# 1. wait up to 40 min for the R22 classification line
LINE=""
for i in $(seq 1 240); do
    LINE=$(grep "round 22: R22" run/fleet/logs/ltp-a-rounds.log 2>/dev/null | tail -1)
    [ -n "$LINE" ] && break
    sleep 10
done
if [ -z "$LINE" ]; then
    echo "[$(date +%H:%M:%S)] GATE TIMEOUT — no R22 line; aborting rollout" >> $LOG
    exit 1
fi
echo "[$(date +%H:%M:%S)] R22: $LINE" >> $LOG
if echo "$LINE" | grep -qE "KERNPANIC|PANIC"; then
    echo "[$(date +%H:%M:%S)] GATE FAIL (panic in R22) — rollout aborted, fleet stays on v5" >> $LOG
    exit 1
fi
echo "[$(date +%H:%M:%S)] GATE PASS — rolling v7" >> $LOG

# 2. riscv members: stop, wait for driver exit, relaunch with KIMG=v7
for m in ltp-b ltp-c ltp-d memstorm fsfuzz; do
    touch run/fleet/$m.stop
done
for m in ltp-b ltp-c ltp-d memstorm fsfuzz; do
    for i in $(seq 1 200); do
        pgrep -f "fleet-member.sh $m[ ]" >/dev/null || break
        sleep 10
    done
    if pgrep -f "fleet-member.sh $m[ ]" >/dev/null; then
        echo "[$(date +%H:%M:%S)] $m did not exit in time; skipping" >> $LOG
        continue
    fi
    KIMG=$V7 ./run/fleet/stress-fleet.sh start $m >> $LOG 2>&1
    echo "[$(date +%H:%M:%S)] $m relaunched on v7" >> $LOG
done

# 3. swap default images (future restarts get v7)
cp run/fleet/images/fleet-rv64-v7-merged-rux run/fleet/images/fleet-rv64-rux
cp run/fleet/images/fleet-x86-v7-merged-bzImage run/fleet/images/fleet-x86-bzImage
echo "[$(date +%H:%M:%S)] default images swapped to v7" >> $LOG

# 4. monkey members: restart on the swapped x86 image
for m in monkey-a monkey-b; do touch run/fleet/$m.stop; done
for m in monkey-a monkey-b; do
    for i in $(seq 1 200); do
        pgrep -f "fleet-member.sh $m[ ]" >/dev/null || break
        sleep 10
    done
    ./run/fleet/stress-fleet.sh start $m >> $LOG 2>&1
    echo "[$(date +%H:%M:%S)] $m relaunched on v7-x86" >> $LOG
done
echo "[$(date +%H:%M:%S)] rollout-v7 complete" >> $LOG
