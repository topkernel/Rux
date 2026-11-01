#!/bin/bash
# golden-watch.sh — watch one golden-population boot; log ladder milestones.
# Usage: golden-watch.sh <tag> — prints verdict lines to run/roll-logs/gold-<tag>.log
TAG=$1
SER="run/oh-${TAG}-serial.log"
OUT="run/roll-logs/gold-${TAG}.log"
DRV=$(pgrep -f "^python3 run/lb-boot.py $TAG " | head -1)
echo "[watch] $TAG driver=$DRV start $(date +%H:%M:%S)" >> "$OUT"
T0=$(date +%s)
while kill -0 "$DRV" 2>/dev/null; do
  sleep 5
  EL=$(( $(date +%s) - T0 ))
  for k in 'sdc result' 'bundles.ready SET' 'insert SA:401' 'insert SA:5003' 'GenerateUserKeys' 'CreateNewUser' 'bundle size' 'SwitchToUser' 'launcher spawn' 'appspawn.*launcher' 'userId not exist' 'PIXELS'; do :; done
  # milestone lines (once each)
  grep -aq 'bundle size' "$SER" && ! grep -aq 'GOLDEN' "$OUT" && { grep -a -m1 'bundle size' "$SER" | cut -c1-160 >> "$OUT"; echo "[watch] $TAG *** GOLDEN at ${EL}s ***" >> "$OUT"; }
  grep -aq 'SwitchToUser' "$SER" && ! grep -aq 'SWITCHED' "$OUT" && { echo "[watch] $TAG SwitchToUser at ${EL}s" >> "$OUT"; echo SWITCHED >> "$OUT"; }
  grep -aqE 'appspawn start com.ohos.launcher|AppSpawn start com.ohos.launcher' "$SER" && ! grep -aq 'SPAWNED' "$OUT" && { echo "[watch] $TAG LAUNCHER SPAWN at ${EL}s" >> "$OUT"; echo SPAWNED >> "$OUT"; }
  grep -aq 'bootevent.launcher.ready' "$SER" && ! grep -aq 'READY' "$OUT" && { echo "[watch] $TAG launcher.ready at ${EL}s" >> "$OUT"; echo READY >> "$OUT"; }
  # service-cadence heartbeat every 120s
  if [ $((EL % 120)) -lt 6 ]; then
    N=$(grep -a -c 'ServiceStart starting' "$SER" 2>/dev/null)
    SA=$(grep -a -c 'insert SA:' "$SER" 2>/dev/null)
    B401=$(grep -a -c 'insert SA:401' "$SER" 2>/dev/null)
    echo "[watch] $TAG ${EL}s svcs=$N sa=$SA bms401=$B401" >> "$OUT"
    sleep 6
  fi
done
echo "[watch] $TAG driver exited at $(( $(date +%s) - T0 ))s $(date +%H:%M:%S)" >> "$OUT"
