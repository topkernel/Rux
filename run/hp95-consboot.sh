#!/bin/bash
# riscv64 minimal-rootfs regression boot (95th merge-gate tier):
# console /bin/sh, then cd /proc + ls + marker echo, no panic.
cd /home/william/rux-agents/oh-crash
{ sleep 25; printf 'cd /proc\n'; sleep 3; printf 'ls\n'; sleep 3; printf 'echo REGRESSION-OK-$?-\n'; sleep 5; printf 'poweroff -f\n'; sleep 10; } | timeout 150 env PLATFORM=riscv64 ./scripts/test/run.sh console /bin/sh > run/hp95-consboot.log 2>&1
echo "rc=$?" >> run/hp95-consboot.log
