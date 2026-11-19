#!/bin/bash
# vma-storm-probe.sh — suite wrapper for the VMA churn storm
# (test/vma_storm.c via test/vma_storm_boot.py): bounded 30 s storm,
# verdict from the driver.
#
# Usage: test/vma-storm-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

leg_run_driver test/vma_storm_boot.py --kernel "$LEG_KERNEL" \
    --secs 30 --timeout "$LEG_TIMECAP"
