#!/bin/bash
# exitwalk-x86-probe.sh — suite wrapper for the exitwalk_boot.py probe leg (x86_64).
#
# Usage: test/exitwalk-x86-probe.sh   (env: LEG_X86_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

leg_run_x86_driver test/exitwalk_boot.py --kernel "$LEG_X86_KERNEL" --mem 2G --secs 30
