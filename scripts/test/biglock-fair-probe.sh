#!/bin/bash
# biglock-fair-probe.sh — suite wrapper for the EXT4_BIG_LOCK fairness
# probe (test/biglock_fair.c): many tenants drive page-cache MISS reads
# through ext4 at SMP2 while a 10 Hz canary reader asserts bounded
# latency — the gate for the R84 handoff/fairness fix.
#
# Usage: test/biglock-fair-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=600}"

NAME=biglock_fair
BIN="$LEG_ROOT/work/leg-$NAME"
GUEST="$LEG_ROOT/work/leg-$NAME.guest"
leg_build_musl "test/$NAME.c" "$BIN" -pthread
cat > "$GUEST" <<'EOF'
echo ===LEG: start
/test/biglock_fair
echo LEG-RC:$?
echo ===LEG: done
EOF
leg_boot_rootfs "$NAME" "$BIN" 45 "$GUEST"
leg_panic_check
leg_boot_ok "===LEG: start"
if grep -aq "BIGFAIR-FAIL\|BIGFAIR-MMAP-FAIL\|BIGFAIR-FAIL-SETUP" "$LEG_LOG"; then
    leg_result FAIL "$(grep -a 'BIGFAIR-' "$LEG_LOG" | grep FAIL | head -1)"
fi
if grep -aq "BIGFAIR-PASS" "$LEG_LOG"; then
    leg_result PASS "BIGFAIR-PASS seen"
fi
leg_result FAIL "no BIGFAIR verdict in serial log"
