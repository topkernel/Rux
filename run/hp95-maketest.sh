#!/bin/bash
# riscv64 make test (95th merge-gate tier: re-verify 1319/1319 at HEAD ca5b9f85)
cd /home/william/rux-agents/oh-crash
PLATFORM=riscv64 make test > run/hp95-maketest.log 2>&1
echo "rc=$?" >> run/hp95-maketest.log
