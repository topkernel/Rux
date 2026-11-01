#!/bin/bash
cd "$(dirname "$0")/.."
STOP=run/pa115p2f-window.stop
rm -f "$STOP"
( while [ ! -f "$STOP" ]; do
    echo "$(date '+%F %T') load1=$(cut -d' ' -f1 /proc/loadavg) qemus=$(pgrep -c qemu-system || echo 0)"
    sleep 60
  done ) > run/pa115p2f-window.log 2>&1 &
MON=$!
python3 test/pbench_boot.py --p2f \
    --bzimage run/pa115-main-afc41b8b-strong-bzImage \
    --accel kvm --tag pa115p2f --timeout 900 \
    > run/pa115p2f-boot.log 2>&1
echo "p2f rc=$?" >> run/pa115p2f-boot.log
touch "$STOP"; wait $MON 2>/dev/null
