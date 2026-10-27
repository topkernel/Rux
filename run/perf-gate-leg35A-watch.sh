#!/bin/bash
# perf-gate-leg35A-watch.sh — wait for the calm window, then run the
# merge-gate perf tiers 1,2 for the leg-35A tree (canonical spawnfix
# ab4ca01c + TXN-POOL census 52535c72; binder domain).
# Polls every 5 min; gives up after 3 h; perf-gate.sh itself refuses
# at >=2 foreign QEMUs (host-capacity discipline), so launch only when
# it accepts.
cd /home/william/rux-agents/oh-kernel || exit 1
DEADLINE=$(( $(date +%s) + 3*3600 ))
TAG=leg35A
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
    n=$(pgrep -c qemu-system 2>/dev/null || echo 0)
    if [ "${n:-9}" -lt 2 ]; then
        echo "$(date +%H:%M:%S) calm (qemu=$n) — launching perf-gate tiers 1,2" \
            >> run/perf-gate-leg35A-watch.log
        bash run/perf-gate.sh --tiers 1,2 --tag "$TAG" \
            > run/perf-gate-leg35A.log 2>&1
        rc=$?
        echo "$(date +%H:%M:%S) perf-gate rc=$rc" >> run/perf-gate-leg35A-watch.log
        exit 0
    fi
    sleep 300
done
echo "$(date +%H:%M:%S) no calm window in 3 h — giving up" \
    >> run/perf-gate-leg35A-watch.log
exit 2
