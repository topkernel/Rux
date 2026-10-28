#!/bin/bash
# decode-x86-probe.sh — suite wrapper: decode_probe_boot.py (x86_64). Materializes the musl
# loader the driver's fixed /tmp/ohroot path expects (musl: the
# loader IS libc.so; in-tree source run/oh-x/rd/lib/).

set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=probe-common.sh
source "$HERE/probe-common.sh"
: "${LEG_TIMECAP:=300}"

leg_need_x86_kernel
leg_x86_musl_loader || leg_result SKIP "musl loader source run/oh-x/rd/lib/ld-musl-x86_64.so.1 absent (loader-dependent driver cannot build)"

# The probe stages the OH lib2d_graphics dependency closure: it reads
# them from a system-image layout. The default is a loop mount that is
# never up in unattended runs; the OH lanes keep a matching syslib tree
# under /tmp — use it when present, SKIP honestly when not.
DECODE_SRC=""
for d in /tmp/oh-legs/syslib64b /tmp/ohroot-sys; do
    [ -f "$d/lib64/lib2d_graphics.z.so" ] && DECODE_SRC="$d" && break
done
[ -n "$DECODE_SRC" ] || leg_result SKIP "no OH syslib tree with lib2d_graphics.z.so (need the OH lanes' /tmp staging or a /mnt/ccsys/system loop mount)"

# The staged syslib's lib64/libc.so is a symlink to ../lib/ld-musl-x86_64.so.1
# and that target is absent in the /tmp copy — repair it from the same
# in-tree loader source (musl: the loader IS libc).
if [ -L "$DECODE_SRC/lib64/libc.so" ] && [ ! -e "$DECODE_SRC/lib64/libc.so" ]; then
    mkdir -p "$DECODE_SRC/lib"
    cp "$LEG_ROOT/run/oh-x/rd/lib/ld-musl-x86_64.so.1" "$DECODE_SRC/lib/ld-musl-x86_64.so.1" \
        || leg_result SKIP "cannot repair the syslib loader symlink target"
fi
export DECODE_SYSROOT="$DECODE_SRC"

leg_run_x86_driver test/decode_probe_boot.py
