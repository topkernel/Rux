#!/bin/bash
# Lock-aware make test: wait for the shared test/rootfs.img (the main
# session's merge gate holds it legitimately), then run the suite.
cd /home/william/rux-agents/ltp-r21
for i in $(seq 1 80); do
  [ -z "$(fuser test/rootfs.img 2>/dev/null | tr -d ' ')" ] && break
  sleep 20
done
timeout 1500 make test > run/maketest-r26-1.log 2>&1
rc=$?
grep -E "Total:|Passed:|Failed:" run/maketest-r26-1.log | tail -3
echo "R26 MAKETEST rc=$rc"
