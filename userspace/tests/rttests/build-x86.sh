#!/bin/bash
# build-x86.sh — extract + static-build the rt-tests suite for the
# qa-suites lane (x86_64 guest, serial console).
#
# Source of truth: rt-tests-v2.11.tar.gz — upstream kernel.org cgit
# snapshot (git.kernel.org/pub/scm/utils/rt-tests/rt-tests.git, tag
# v2.11), committed unmodified. This script extracts it into scratch
# (run/rttests-build/) and builds the three round-1 binaries without
# touching the upstream tree, plus one host-glibc build:
#
#   run/rttests-bin/hackbench   (musl static)
#   run/rttests-bin/cyclictest  (musl static)
#   run/rttests-bin/pi_stress   (host glibc static — musl reports
#                                _POSIX_THREAD_PRIO_INHERIT == -1 and
#                                upstream pi_stress refuses to build on
#                                it; the glibc build carries the real
#                                FUTEX_LOCK_PI machinery)
#
# The libnuma shim (shim-numa.h/.c) provides the small libnuma API
# subset rt-tests links against, reporting a single NUMA node.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/../../.." && pwd)"
SCRATCH="${SCRATCH:-$ROOT/run/rttests-build}"
OUT="${OUT:-$ROOT/run/rttests-bin}"
MUSL_CC="$ROOT/toolchain/x86_64-rux-linux-musl/bin/musl-gcc"
VERSION=2.11
TARBALL="rt-tests-v$VERSION.tar.gz"

[ -x "$MUSL_CC" ] || {
    echo "error: $MUSL_CC missing — extract toolchain/x86_64-rux-linux-musl.tar.gz first" >&2
    exit 1
}

mkdir -p "$SCRATCH" "$OUT"
rm -rf "$SCRATCH/rt-tests-v$VERSION"
tar xzf "$DIR/$TARBALL" -C "$SCRATCH"
SRC="$SCRATCH/rt-tests-v$VERSION"

# Ship the numa shim under the name rt-tests includes (<numa.h>).
cp "$DIR/shim-numa.h" "$SRC/numa.h"

# Patch 1 (compat, applied to the scratch copy only): upstream redefines
# sigev_notify_thread_id as the glibc _sigev_un._tid union member; musl
# already provides this macro, and the redefine breaks the build there.
# Guarding it is correct for both libcs.
sed -i 's|^#define sigev_notify_thread_id _sigev_un._tid$|#ifndef sigev_notify_thread_id\n#define sigev_notify_thread_id _sigev_un._tid\n#endif|' \
    "$SRC/src/cyclictest/cyclictest.c"

# -Wno-error=*: upstream predates GCC-14/15 promoting these to errors
# (K&R-ish socket casts and implicit decls); keep them as warnings.
# The musl toolchain's libc.a ships ENOSYS stubs for the sched_* family
# (qa-suites round-1 finding); real syscall wrappers linked ahead of the
# archive override them. See x86-sched-wrappers.c.
"$MUSL_CC" -static -O2 -c -o "$OUT/x86-sched-wrappers.o" "$DIR/x86-sched-wrappers.c"
WRAP="$OUT/x86-sched-wrappers.o"

CFLAGS="-O2 -Wall -D_GNU_SOURCE -DVERSION=$VERSION -I$SRC -I$SRC/src/include -I$SRC/src/cyclictest -Wno-error=incompatible-pointer-types -Wno-error=implicit-function-declaration -Wno-error=int-conversion"
LIBS="$SRC/src/lib/rt-utils.c $SRC/src/lib/rt-error.c $SRC/src/lib/rt-get_cpu.c $SRC/src/lib/rt-sched.c $SRC/src/lib/histogram.c"

echo "== hackbench (musl static)"
"$MUSL_CC" -static $CFLAGS -o "$OUT/hackbench" "$SRC/src/hackbench/hackbench.c" "$WRAP"

echo "== cyclictest (musl static, numa shim)"
"$MUSL_CC" -static $CFLAGS -o "$OUT/cyclictest" "$SRC/src/cyclictest/cyclictest.c" \
    "$SRC/src/lib/rt-numa.c" "$DIR/shim-numa.c" $LIBS "$WRAP"

echo "== pi_stress (host glibc static — FUTEX_LOCK_PI path)"
gcc -static $CFLAGS -o "$OUT/pi_stress" "$SRC/src/pi_tests/pi_stress.c" $LIBS -lpthread

file "$OUT"/hackbench "$OUT"/cyclictest "$OUT"/pi_stress
echo "binaries in $OUT"
