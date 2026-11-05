#!/bin/bash
# filedrain-probe.sh — suite wrapper for the page-cache drain behavior
# probe (test/filedrain.c via test/filedrain_boot.py): one bounded
# soak boot, FDR-PASS marker decides.
#
# The driver's own default window (1500 s) exceeds the suite's probe
# leg budget (900 s); 600 s keeps boot + workload inside it — the
# workload is window-driven, so a shorter window still produces its
# verdict.
#
# Usage: test/filedrain-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

leg_run_driver test/filedrain_boot.py --kernel "$LEG_KERNEL" --timeout 600
