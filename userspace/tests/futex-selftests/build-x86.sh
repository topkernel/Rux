#!/bin/bash
# build-x86.sh — static-musl build of the reference kernel's futex
# selftests (tools/testing/selftests/futex, kernel linux-6.6) for the
# x86_64 guest (qa-suites round 6; user directive 2026-11-05: onboarding
# OH-side test cases for the ffrt/binder/futex pain).
#
# Source of truth: futex-selftests.tar.gz — the upstream futex/ tree
# (functional/ + include/ + Makefile + run.sh) plus the selftests
# kselftest.h, committed unmodified from the reference kernel tree.
#
# musl gaps this script shims:
#   - kernel UAPI headers (linux/futex.h incl. struct futex_waitv):
#     staged wholesale from the host's linux-libc-dev UAPI subdirs (the
#     fio build's stage-0 pattern — never /usr/include wholesale, no
#     host glibc root headers leak into the musl link)
#   - <error.h> (glibc-only; only futex_waitv.c includes it — shimmed)
#   - __NR_futex_waitv (449 on x86_64; #ifndef fallback via -include)
#
# Output: run/futex-selftests-bin/<test> — one static binary per
# functional test. Verdict protocol: kselftest TAP lines + exit code.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/../../.." && pwd)"
SCRATCH="${SCRATCH:-$ROOT/run/futex-selftests-build}"
OUT="${OUT:-$ROOT/run/futex-selftests-bin}"
MUSL_CC="$ROOT/toolchain/x86_64-rux-linux-musl/bin/musl-gcc"

[ -x "$MUSL_CC" ] || { echo "error: $MUSL_CC missing — extract toolchain first" >&2; exit 1; }

# ---- stage 0: kernel UAPI headers into a private prefix (fio pattern) ----
UAPI_PREFIX="$SCRATCH/linux-uapi"
rm -rf "$UAPI_PREFIX"
mkdir -p "$UAPI_PREFIX" "$SCRATCH" "$OUT"
cp -r /usr/include/linux "$UAPI_PREFIX/linux"
cp -r /usr/include/x86_64-linux-gnu/asm "$UAPI_PREFIX/asm"
cp -r /usr/include/asm-generic "$UAPI_PREFIX/asm-generic"

# ---- stage 1: glibc-only + syscall-nr shims ----
SHIM="$SCRATCH/shim"
rm -rf "$SHIM"
mkdir -p "$SHIM"
cat > "$SHIM/error.h" <<'EOF'
/* musl shim for the glibc-only <error.h> (only futex_waitv.c includes it) */
#ifndef _GLIBC_ERROR_SHIM_H
#define _GLIBC_ERROR_SHIM_H
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
static inline void error(int status, int errnum, const char *fmt, ...)
{
    va_list ap; va_start(ap, fmt);
    if (errnum) fprintf(stderr, "error: %s: ", strerror(errnum));
    else fprintf(stderr, "error: ");
    vfprintf(stderr, fmt, ap);
    va_end(ap);
    if (status) exit(status);
}
#endif
EOF
cat > "$SHIM/nr_compat.h" <<'EOF'
/* futex_waitv may postdate the staged UAPI's unistl tables */
#ifndef __NR_futex_waitv
#define __NR_futex_waitv 449
#endif
EOF

# ---- stage 2: extract + build ----
rm -rf "$SCRATCH/futex"
tar xzf "$DIR/futex-selftests.tar.gz" -C "$SCRATCH"
FUX="$SCRATCH/futex"

# kselftest.h sits at the tarball root (one level above futex/).
# -include libgen.h: musl keeps basename()'s prototype there only; the
# upstream tests call it unguarded (glibc declares it in string.h too).
CFLAGS="-O2 -Wall -D_GNU_SOURCE -I$SHIM -I$UAPI_PREFIX -include $SHIM/nr_compat.h -include libgen.h -I$FUX/include -I$SCRATCH"

for src in "$FUX"/functional/*.c; do
    name="$(basename "$src" .c)"
    # shellcheck disable=SC2086
    "$MUSL_CC" -static $CFLAGS -lpthread -o "$OUT/$name" "$src"
done

echo "binaries:"
for b in "$OUT"/*; do file "$b" | sed 's|, with debug_info.*||;s|, not stripped||'; done | sed "s|$OUT/||"
echo "output in $OUT"
