#!/bin/bash
# r5b-fio-battery.sh — round-5b: F-6 posixaio hunt + extended stress classes
# All hunt boots carry syscost=1 (census for the monopolist identity).
set -uo pipefail
cd "$(dirname "$0")/.."
export OUTDIR=run/qa-runs-r5b
export DFX="${DFX:-dfx=taskdump,watchdog}"

fio() {
    INJBINS="$PWD/run/fio-bin" BUDGET="$1" EXTRA="${EXTRA:-}" \
        test/qrtests/run-suite-x86.sh -f "run/$2" 2>&1 \
        | grep -aE "RES\||boot:" | cut -c1-220
}

echo "=== r5b hunt (F-6 posixaio) ==="
export EXTRA="syscost=1"
fio 350 r5b-hunt1.cmds
fio 450 r5b-hunt2.cmds
fio 450 r5b-hunt3.cmds
fio 500 r5b-hunt4.cmds
unset EXTRA
echo "=== r5b extended classes ==="
fio 500 r5b-churn.cmds
fio 700 r5b-bigfile.cmds
fio 500 r5b-fsyncburn.cmds
fio 500 r5b-deepdirect.cmds
echo "=== r5b battery done ==="
