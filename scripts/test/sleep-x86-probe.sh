#!/bin/bash
# sleep-x86-probe.sh — suite wrapper for the sleep-precision oracle
# (test/sleep_probe.c via test/sleep_probe_boot.py, x86_64): usleep
# jitter statistics as init. Materializes the /tmp/ohroot musl loader
# the driver's dynamic-link build expects.
#
# Usage: test/sleep-x86-probe.sh   (env: LEG_X86_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

leg_need_x86_kernel
leg_x86_musl_loader || leg_result SKIP "musl loader source run/oh-x/rd/lib/ld-musl-x86_64.so.1 absent (driver cannot build)"

leg_run_x86_driver test/sleep_probe_boot.py
