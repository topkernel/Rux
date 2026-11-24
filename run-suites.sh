#!/bin/bash
# Suite battery for the fix tree: rootfs prep if needed, riscv64 suite,
# then x86_64 suite.
set -u
cd /home/william/rux-agents/novma-p0-fix
if [ ! -f test/rootfs.img ]; then
    make user > run/suite-prep.log 2>&1 && make rootfs >> run/suite-prep.log 2>&1
fi
make test > run/suite-rv.log 2>&1
echo "rv-rc=$?" >> run/suite-rv.log
PLATFORM=x86_64 make test > run/suite-x86.log 2>&1
echo "x86-rc=$?" >> run/suite-x86.log
