#!/bin/bash
# build-x86.sh — extract + static-build the portable kselftest subset for
# the qa-suites lane (x86_64 guest, serial console).
#
# Source of truth: kselftest-v6.6-subset.tar.gz — individual files fetched
# verbatim from the reference kernel v6.6 tag via kernel.org cgit (tools/
# testing/selftests/{kselftest.h,memfd,timers} + the three include/uapi/
# linux headers musl does not ship: falloc.h memfd.h fcntl.h). Assembled
# into the tarball by the round-2 onboarding; committed unmodified.
#
# Output: run/kselftest-bin/<name> — one static musl binary per test:
#   memfd            memfd_test.c + common.c (seals, mmap, shrink/grow)
#   posix_timers nanosleep nsleep-lat set-timer-lat mqueue-lat
#   inconsistency-check raw_skew threadtest        (timers family)
#
# Skipped from upstream timers/TEST_GEN_PROGS: rtcpie (needs /dev/rtc),
# DESTRUCTIVE_TESTS (adjtimex tick surgery — wrong for a shared rootfs).
# Skipped from memfd/: fuse_mnt/fuse_test (FUSE not in scope).
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/../../.." && pwd)"
SCRATCH="${SCRATCH:-$ROOT/run/kselftest-build}"
OUT="${OUT:-$ROOT/run/kselftest-bin}"
MUSL_CC="$ROOT/toolchain/x86_64-rux-linux-musl/bin/musl-gcc"
TARBALL="kselftest-v6.6-subset.tar.gz"

[ -x "$MUSL_CC" ] || {
    echo "error: $MUSL_CC missing — extract toolchain/x86_64-rux-linux-musl.tar.gz first" >&2
    exit 1
}

mkdir -p "$SCRATCH" "$OUT"
rm -rf "$SCRATCH/kselftest-v6.6-subset"
tar xzf "$DIR/$TARBALL" -C "$SCRATCH"
SRC="$SCRATCH/kselftest-v6.6-subset"

CFLAGS="-O2 -Wall -static -D_GNU_SOURCE -I$SRC -I$SRC/uapi"

# memfd: upstream expects <linux/falloc.h>/<linux/memfd.h> from the
# kernel's exported uapi; the tarball ships them under uapi/ (musl
# fcntl.h already carries FALLOC_FL_* — uapi/fcntl.h defines none, so
# the two coexist).
echo "== memfd"
"$MUSL_CC" $CFLAGS -o "$OUT/memfd" "$SRC/memfd/memfd_test.c" "$SRC/memfd/common.c"

# Patch 1 (compat, applied to the scratch copy only): set-timer-lat.c
# calls select() but only includes <sys/time.h>-side headers; glibc
# declares select from <unistd.h> with _GNU_SOURCE, musl does not.
# Adding <sys/select.h> is correct for both.
sed -i 's|^#include <errno.h>$|#include <errno.h>\n#include <sys/select.h>|' \
    "$SRC/timers/set-timer-lat.c"
# Same class for mqueue-lat.c: uses O_CREAT/O_RDONLY; glibc's include
# chain provides fcntl.h constants, musl needs the explicit include.
sed -i 's|^#include <errno.h>$|#include <errno.h>\n#include <fcntl.h>|' \
    "$SRC/timers/mqueue-lat.c"

echo "== timers family"
for t in posix_timers nanosleep nsleep-lat set-timer-lat mqueue-lat \
         inconsistency-check raw_skew threadtest; do
    "$MUSL_CC" $CFLAGS -o "$OUT/$t" "$SRC/timers/$t.c"
done

file "$OUT"/memfd "$OUT"/posix_timers | sed 's/, BuildID.*//'
echo "binaries in $OUT"
