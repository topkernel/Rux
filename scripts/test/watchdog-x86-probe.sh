#!/bin/bash
# watchdog-x86-probe.sh — suite wrapper for the watchdog_probe_boot.py probe leg (x86_64).
#
# Usage: test/watchdog-x86-probe.sh   (env: LEG_X86_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

leg_run_x86_driver test/watchdog_probe_boot.py --mem 2G
