#!/bin/bash
# megapage-mmap-probe.sh — suite wrapper for the megapage demotion E2E
# probe (test/megapage_mmap.c): fixed-address 4K mmap over a cloned 2MB
# megapage, touch/munmap/re-map — the gnome-shell heap-cage repro.
# Builds the probe, injects it into a private rootfs copy, boots and
# asserts the megapage_mmap PASS lines.
#
# Usage: test/megapage-mmap-probe.sh   (env: LEG_KERNEL, LEG_TIMECAP)

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

NAME=megapage_mmap
BIN="$LEG_ROOT/work/leg-$NAME"
GUEST="$LEG_ROOT/work/leg-$NAME.guest"
leg_build_musl "test/$NAME.c" "$BIN"
cat > "$GUEST" <<'EOF'
echo ===LEG: start
/test/megapage_mmap
echo LEG-RC:$?
echo ===LEG: done
EOF
leg_boot_rootfs "$NAME" "$BIN" 45 "$GUEST"
leg_panic_check
leg_boot_ok "===LEG: start"
if grep -aq "megapage_mmap: FAIL" "$LEG_LOG"; then
    leg_result FAIL "$(grep -a 'megapage_mmap: FAIL' "$LEG_LOG" | head -1)"
fi
if grep -aq "megapage_mmap: PASS" "$LEG_LOG"; then
    leg_result PASS "megapage_mmap PASS lines seen"
fi
leg_result FAIL "no megapage_mmap verdict in serial log"
