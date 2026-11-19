#!/bin/bash
# reclaimwb-probe.sh — suite wrapper for the reclaim writeback probe
# (test/reclaimwb.c via test/reclaimwb_boot.py, riscv64 leg): one
# bounded soak boot under memory pressure.
#
# Driver default window is 1800 s; 600 s keeps the leg inside the
# suite's 900 s probe budget (the workload is window-driven).
#
# Usage: test/reclaimwb-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

leg_run_driver test/reclaimwb_boot.py --platform riscv64 \
    --kernel "$LEG_KERNEL" --timeout 600
