#!/bin/bash
# nettest-probe.sh — suite wrapper for the loopback network E2E probe
# (test/nettest.c): raw-syscall UDP/TCP echo over the loopback device,
# plus vfork/clone redirect coverage.
#
# Usage: test/nettest-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

NAME=nettest
BIN="$LEG_ROOT/work/leg-$NAME"
GUEST="$LEG_ROOT/work/leg-$NAME.guest"
leg_build_free "test/$NAME.c" "$BIN"
cat > "$GUEST" <<'EOF'
echo ===LEG: start
/test/nettest
echo LEG-RC:$?
echo ===LEG: done
EOF
leg_boot_rootfs "$NAME" "$BIN" 45 "$GUEST"
leg_panic_check
leg_boot_ok "===LEG: start"
if grep -aq "NETTEST FAIL" "$LEG_LOG"; then
    leg_result FAIL "$(grep -a 'NETTEST FAIL' "$LEG_LOG" | head -1)"
fi
if grep -aq "NETTEST PASS" "$LEG_LOG"; then
    leg_result PASS "NETTEST PASS seen"
fi
leg_result FAIL "no NETTEST verdict in serial log"
