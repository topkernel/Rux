#!/bin/bash
# filemap-probe.sh — suite wrapper for the file-backed mmap coherence
# probe (test/filemap.c via test/filemap_boot.py, riscv64 leg).
#
# Usage: test/filemap-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

leg_run_driver test/filemap_boot.py --platform riscv64 \
    --kernel "$LEG_KERNEL" --timeout "$LEG_TIMECAP"
