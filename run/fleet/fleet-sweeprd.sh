#!/bin/bash
# fleet-sweeprd.sh — periodic central sweep loop for the stress fleet
# (started by stress-fleet.sh under setsid; kill by pkill -f
# fleet-sweeprd.sh — it owns no QEMUs).
FLEET="$(cd "$(dirname "$0")" && pwd)"
while :; do
    "$FLEET/fleet-sweep.sh" >/dev/null 2>&1
    sleep 1800
done
