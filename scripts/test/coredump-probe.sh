#!/bin/bash
# coredump-probe.sh — suite wrapper for the core-dump E2E probe
# (test/coredump_probe.c): raises RLIMIT_CORE, takes a NULL-deref
# SIGSEGV; the leg asserts the child died by signal and a core file
# landed in /tmp (the kernel's core_pattern default).
#
# Usage: test/coredump-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

NAME=coredump_probe
BIN="$LEG_ROOT/work/leg-$NAME"
GUEST="$LEG_ROOT/work/leg-$NAME.guest"
leg_build_musl "test/$NAME.c" "$BIN" -g
cat > "$GUEST" <<'EOF'
echo ===LEG: start
cd /tmp
rm -f core*
/test/coredump_probe
echo LEG-RC:$?
ls -l /tmp/core* 2>/dev/null
echo LEG-CORES:$(ls /tmp/core* 2>/dev/null | wc -l)
echo ===LEG: done
EOF
leg_boot_rootfs "$NAME" "$BIN" 45 "$GUEST"
leg_panic_check
leg_boot_ok "===LEG: start"
if grep -aq "probe: ERROR - survived NULL deref" "$LEG_LOG"; then
    leg_result FAIL "probe survived the NULL dereference"
fi
if grep -aqE "LEG-CORES:[1-9]" "$LEG_LOG"; then
    leg_result PASS "SIGSEGV death produced a core file"
fi
if grep -aq "LEG-RC:1[13][0-9]" "$LEG_LOG"; then
    # died by SIGSEGV (139 = 128+11) but no core landed
    leg_result FAIL "probe died by SIGSEGV but no core file was written"
fi
leg_result FAIL "no coredump verdict in serial log"
