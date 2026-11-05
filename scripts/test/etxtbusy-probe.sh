#!/bin/bash
# etxtbusy-probe.sh — suite wrapper for the ETXTBSY exec gate probe
# (test/etxtbusy.c): execve must fail ETXTBSY while an open file
# description holds the image for writing, and succeed after the writer
# closes (Linux deny_write_access; LTP execve04 shape — execve04 is not
# in the fleet sweep list, so this leg owns the coverage).
#
# Usage: test/etxtbusy-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=420}"

NAME=etxtbusy
BIN="$LEG_ROOT/work/leg-$NAME"
GUEST="$LEG_ROOT/work/leg-$NAME.guest"
leg_build_musl "test/$NAME.c" "$BIN"
cat > "$GUEST" <<'EOF'
echo ===LEG: start
/test/etxtbusy
echo LEG-RC:$?
echo ===LEG: done
EOF
leg_boot_rootfs "$NAME" "$BIN" 45 "$GUEST"
leg_panic_check
leg_boot_ok "===LEG: start"
if grep -aq "ETXTBUSY done" "$LEG_LOG"; then
    leg_result PASS "ETXTBUSY done seen ($(grep -a 'ETXTBUSY done' "$LEG_LOG" | head -1))"
fi
leg_result FAIL "etxtbusy never completed (no ETXTBUSY done marker)"
