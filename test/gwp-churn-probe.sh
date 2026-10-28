#!/bin/bash
# gwp-churn-probe.sh — suite wrapper for the guard-page write churn
# probe (test/gwp_churn.c): 20 s of mprotect/fault churn; any
# GWPFAIL-* line fails, GWP-PASS closes the leg.
#
# Usage: test/gwp-churn-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

NAME=gwp_churn
BIN="$LEG_ROOT/work/leg-$NAME"
GUEST="$LEG_ROOT/work/leg-$NAME.guest"
leg_build_musl "test/$NAME.c" "$BIN" -pthread
cat > "$GUEST" <<'EOF'
echo ===LEG: start
/test/gwp_churn 20
echo LEG-RC:$?
echo ===LEG: done
EOF
leg_boot_rootfs "$NAME" "$BIN" 45 "$GUEST"
leg_panic_check
leg_boot_ok "===LEG: start"
if grep -aq "GWPFAIL-" "$LEG_LOG"; then
    leg_result FAIL "$(grep -a 'GWPFAIL-' "$LEG_LOG" | head -1)"
fi
if grep -aq "GWP-PASS" "$LEG_LOG"; then
    leg_result PASS "GWP-PASS seen"
fi
leg_result FAIL "no GWP verdict in serial log"
