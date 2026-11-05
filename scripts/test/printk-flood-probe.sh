#!/bin/bash
# printk-flood-probe.sh — suite wrapper for the printk wedge flood
# probe (test/printk_flood.c): drives MREMAP-CFI kernel printk flood
# and direct console-write pressure; FLOOD-DONE OK closes the leg.
#
# Usage: test/printk-flood-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=780}"

NAME=printk_flood
BIN="$LEG_ROOT/work/leg-$NAME"
GUEST="$LEG_ROOT/work/leg-$NAME.guest"
# 240 s guest flood (~300 s host wall under TCG load): keeps the leg
# inside the suite's 900 s probe budget while still driving >1M flood
# lines through printk and the console.
leg_build_free "test/$NAME.c" "$BIN" -DFLOOD_SECS=240
cat > "$GUEST" <<'EOF'
echo ===LEG: start
/test/printk_flood
echo LEG-RC:$?
echo ===LEG: done
EOF
leg_boot_rootfs "$NAME" "$BIN" 45 "$GUEST"
leg_panic_check
leg_boot_ok "===LEG: start"
if grep -aq "FLOOD-DONE FAIL" "$LEG_LOG"; then
    leg_result FAIL "FLOOD-DONE FAIL seen"
fi
if grep -aq "FLOOD-DONE OK" "$LEG_LOG"; then
    leg_result PASS "FLOOD-DONE OK seen"
fi
leg_result FAIL "no FLOOD-DONE verdict in serial log"
