#!/bin/bash
# cfi-shadow-probe.sh — suite wrapper for the cross-DSO CFI shadow init
# probe (test/cfi_shadow.c): replicates ld-musl's PROT_NONE reserve +
# mprotect + mremap(MREMAP_FIXED) fill sequence — the "Permission
# denied at __cfi_slowpath" SIGSEGV family.
#
# Usage: test/cfi-shadow-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

NAME=cfi_shadow
BIN="$LEG_ROOT/work/leg-$NAME"
GUEST="$LEG_ROOT/work/leg-$NAME.guest"
leg_build_musl "test/$NAME.c" "$BIN" -pthread
cat > "$GUEST" <<'EOF'
echo ===LEG: start
/test/cfi_shadow
echo LEG-RC:$?
echo ===LEG: done
EOF
leg_boot_rootfs "$NAME" "$BIN" 45 "$GUEST"
leg_panic_check
leg_boot_ok "===LEG: start"
if grep -aq "CFI-FAIL" "$LEG_LOG"; then
    leg_result FAIL "$(grep -a 'CFI-FAIL' "$LEG_LOG" | head -1)"
fi
if grep -aq "CFI-PASS" "$LEG_LOG"; then
    leg_result PASS "CFI-PASS seen"
fi
leg_result FAIL "no CFI verdict in serial log"
