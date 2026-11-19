#!/bin/bash
# loader-vma-x86-probe.sh — suite wrapper: loader_vma_boot.py (x86_64). Materializes the musl
# loader the driver's fixed /tmp/ww-loader path expects (musl: the
# loader IS libc.so; in-tree source run/oh-x/rd/lib/).

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

leg_need_x86_kernel
leg_x86_musl_loader || leg_result SKIP "musl loader source run/oh-x/rd/lib/ld-musl-x86_64.so.1 absent (loader-dependent driver cannot build)"

leg_run_x86_driver test/loader_vma_boot.py
