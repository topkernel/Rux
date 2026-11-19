#!/bin/bash
# chrootcwd-probe.sh — suite wrapper for the chroot/getcwd semantics
# probe (test/chrootcwd.c): root-relative getcwd, ENOENT for a jailed
# cwd, in-root opens, cwd self-heal across MS_MOVE.
#
# Usage: test/chrootcwd-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

NAME=chrootcwd
BIN="$LEG_ROOT/work/leg-$NAME"
GUEST="$LEG_ROOT/work/leg-$NAME.guest"
leg_build_free "test/$NAME.c" "$BIN"
cat > "$GUEST" <<'EOF'
echo ===LEG: start
/test/chrootcwd
echo LEG-RC:$?
echo ===LEG: done
EOF
leg_boot_rootfs "$NAME" "$BIN" 45 "$GUEST"
leg_panic_check
leg_boot_ok "===LEG: start"
if grep -aq "CHROOTCWD FAIL" "$LEG_LOG"; then
    leg_result FAIL "$(grep -a 'CHROOTCWD FAIL' "$LEG_LOG" | head -1)"
fi
if grep -aq "CHROOTCWD PASS" "$LEG_LOG"; then
    leg_result PASS "CHROOTCWD PASS seen"
fi
leg_result FAIL "no CHROOTCWD verdict in serial log"
