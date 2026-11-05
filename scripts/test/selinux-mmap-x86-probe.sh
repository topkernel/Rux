#!/bin/bash
# selinux-mmap-x86-probe.sh — suite wrapper for the selinux_mmap driver leg (positional bzImage arg).
#
# Usage: test/selinux-mmap-x86-probe.sh   (env: LEG_X86_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

leg_run_x86_driver test/selinux_mmap_boot.py "$LEG_X86_KERNEL"
