#!/bin/bash
# binder2-probe.sh — suite wrapper for the binder threads + death
# recursion probe (test/binder2_probe.c via test/binder2_boot.py).
#
# Usage: test/binder2-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

leg_run_driver test/binder2_boot.py --kernel "$LEG_KERNEL" --timeout "$LEG_TIMECAP"
