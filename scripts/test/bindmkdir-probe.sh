#!/bin/bash
# bindmkdir-probe.sh — suite wrapper for the bind-mount + mkdir
# visibility probe (test/bindmkdir.c via test/bindmkdir_boot.py).
#
# Usage: test/bindmkdir-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

leg_run_driver test/bindmkdir_boot.py --kernel "$LEG_KERNEL" \
    --timeout "$LEG_TIMECAP"
