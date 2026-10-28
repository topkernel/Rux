#!/bin/bash
# binder-probe.sh — suite wrapper for the Spike S1 binder closed-loop
# probe (test/binder_probe.c via test/binder_probe_boot.py): builds the
# static probe, packs a tiny private ext4, boots it as init and reads
# the BINDER_PROBE RESULT marker off the serial console.
#
# Usage: test/binder-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"

leg_run_driver test/binder_probe_boot.py --kernel "$LEG_KERNEL" --timeout "${LEG_TIMECAP}"
