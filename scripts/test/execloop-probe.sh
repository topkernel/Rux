#!/bin/bash
# execloop-probe.sh — suite wrapper for the pure-exec leak probe
# (test/execloop.c): 100 rounds of fork+execve+wait of a real binary;
# asserts the EXECLOOP done marker and a bounded heap delta (the
# workload that exposed the free_task_slot exe_path page leak).
#
# Usage: test/execloop-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=420}"

NAME=execloop
BIN="$LEG_ROOT/work/leg-$NAME"
GUEST="$LEG_ROOT/work/leg-$NAME.guest"
leg_build_musl "test/$NAME.c" "$BIN"
cat > "$GUEST" <<'EOF'
echo ===LEG: start
/test/execloop 100
echo LEG-RC:$?
echo ===LEG: done
EOF
leg_boot_rootfs "$NAME" "$BIN" 45 "$GUEST"
leg_panic_check
leg_boot_ok "===LEG: start"
if grep -aq "EXECLOOP done" "$LEG_LOG"; then
    leg_result PASS "EXECLOOP done seen ($(grep -a 'EXECLOOP done' "$LEG_LOG" | head -1))"
fi
leg_result FAIL "execloop never completed (no EXECLOOP done marker)"
