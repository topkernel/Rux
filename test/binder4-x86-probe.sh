#!/bin/bash
# binder4-x86-probe.sh — suite wrapper for the x86 binder transaction
# stress probe (test/binder4_probe.c via test/binder4_boot.py; host
# gcc builds the amd64 guest binary, the driver boots the x86 bzImage).
#
# Usage: test/binder4-x86-probe.sh   (env: LEG_X86_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"

: "${LEG_X86_KERNEL:=$LEG_ROOT/target/x86_64-unknown-none/debug/bzImage}"
: "${LEG_TIMECAP:=300}"

[ -f "$LEG_X86_KERNEL" ] || { echo "PROBE-RESULT: SKIP x86 bzImage not built at $LEG_X86_KERNEL (PLATFORM=x86_64 make build)"; exit 3; }
command -v gcc >/dev/null 2>&1 || { echo "PROBE-RESULT: SKIP host gcc not found"; exit 3; }

mkdir -p "$LEG_ROOT/run"
: > "$LEG_LOG"
timeout "$LEG_TIMECAP" python3 "$LEG_ROOT/test/binder4_boot.py" \
    --kernel "$LEG_X86_KERNEL" --mem 2G --timeout "${LEG_TIMECAP}" >> "$LEG_LOG" 2>&1
rc=$?
tail -20 "$LEG_LOG"
case "$rc" in
    0) echo "PROBE-RESULT: PASS driver binder4_boot.py completed cleanly"; exit 0 ;;
    124) echo "PROBE-RESULT: FAIL driver binder4_boot.py timed out"; exit 1 ;;
    *) echo "PROBE-RESULT: FAIL driver binder4_boot.py exited rc=$rc"; exit 1 ;;
esac
