#!/bin/bash
# faultlat3-x86-probe.sh — suite wrapper for the faultlat3_boot.py probe leg (x86_64).
#
# Usage: test/faultlat3-x86-probe.sh   (env: LEG_X86_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=560}"

leg_run_x86_driver test/faultlat3_boot.py --kernel "$LEG_X86_KERNEL" --timeout 500
