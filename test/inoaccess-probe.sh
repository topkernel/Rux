#!/bin/bash
# inoaccess-probe.sh — suite wrapper for the IN_ACCESS event-count probe
# (test/inoaccess.c): read(2)/pread(2)/O_DIRECT pread on a watched ext4
# file must each deliver exactly ONE IN_ACCESS event (Linux fsnotify
# fires once per successful read syscall; the ext4 read_at self-notify
# doubled the wrapper's event).
#
# Usage: test/inoaccess-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=420}"

NAME=inoaccess
BIN="$LEG_ROOT/work/leg-$NAME"
GUEST="$LEG_ROOT/work/leg-$NAME.guest"
leg_build_musl "test/$NAME.c" "$BIN"
cat > "$GUEST" <<'EOF'
echo ===LEG: start
/test/inoaccess
echo LEG-RC:$?
echo ===LEG: done
EOF
leg_boot_rootfs "$NAME" "$BIN" 45 "$GUEST"
leg_panic_check
leg_boot_ok "===LEG: start"
if grep -aq "INOACCESS done" "$LEG_LOG"; then
    leg_result PASS "INOACCESS done seen ($(grep -a 'INOACCESS ok' "$LEG_LOG" | wc -l) rows ok)"
fi
leg_result FAIL "inoaccess rows wrong ($(grep -a 'INOACCESS FAIL' "$LEG_LOG" | head -3))"
