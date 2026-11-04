#!/bin/bash
# r5-fio-battery.sh — round-5 fio stress battery (one family per boot)
# Survey findings baked in: psync/sync/vsync/pvsync2/mmap/splice PASS,
# direct=1 PASS; libaio=ENOSYS(gap), io_uring=SIGSEGV(F-class), posixaio
# =boot wedge (F-class, fdreap grace starvation family).
set -uo pipefail
cd "$(dirname "$0")/.."
export OUTDIR=run/qa-runs-r5
export DFX="${DFX:-dfx=taskdump,watchdog}"

fio() {
    INJBINS="$PWD/run/fio-bin" BUDGET="$1" \
        test/qrtests/run-suite-x86.sh -f "run/$2" 2>&1 \
        | grep -aE "RES\||boot:" | cut -c1-220
}

echo "=== r5 fio battery ==="
fio 500 r5-seqrw.cmds
fio 650 r5-randrw.cmds
fio 650 r5-fsync.cmds
fio 650 r5-mmaprwn.cmds
fio 650 r5-direct.cmds
fio 700 r5-verify.cmds
fio 700 r5-multi.cmds
echo "=== r5 fio battery done ==="
