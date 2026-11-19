#!/bin/bash
# orphan-storm-probe.sh — suite wrapper for the reparent-safety storm
# (test/orphan_storm.c): the default 16 rounds / 10 chains profile —
# bounded below the known pre-existing fork-churn wedge (round ~24+),
# a regression fence for the reparent paths.
#
# Usage: test/orphan-storm-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=480}"

NAME=orphan_storm
BIN="$LEG_ROOT/work/leg-$NAME"
GUEST="$LEG_ROOT/work/leg-$NAME.guest"
leg_build_musl "test/$NAME.c" "$BIN"
cat > "$GUEST" <<'EOF'
echo ===LEG: start
/test/orphan_storm 16 10
echo LEG-RC:$?
echo ===LEG: done
EOF
leg_boot_rootfs "$NAME" "$BIN" 45 "$GUEST"
leg_panic_check
leg_boot_ok "===LEG: start"
if grep -aq "orphan_storm: DONE" "$LEG_LOG"; then
    leg_result PASS "orphan_storm 16 rounds completed"
fi
leg_result FAIL "orphan_storm did not complete (no DONE marker)"
