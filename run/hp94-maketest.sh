#!/bin/bash
# riscv64 make test (merge-gate duty from 91.6#1, inherited via 92nd charter)
cd /home/william/rux-agents/oh-crash
make test > run/hp94-maketest.log 2>&1
echo "rc=$?" >> run/hp94-maketest.log
