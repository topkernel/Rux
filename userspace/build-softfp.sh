#!/bin/bash
# Rux OS - pinned-march soft-fp overlay build (BUG-S005 / S004 hygiene)
#
# Compiles gcc's soft-fp quad-float routines with the project march pin
# (-march=rv64gc_zicsr) so static links can preempt the Ubuntu libgcc.a
# members that were built with zbb+zcb encodings (see soft-fp/README.md).
#
# Output: userspace/soft-fp/libsoftfp-pin.a
#
# Usage: ./build-softfp.sh   (from userspace/, or via its own path)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="${SCRIPT_DIR}/soft-fp"
OUT_A="${SRC}/libsoftfp-pin.a"

if ! command -v riscv64-linux-gnu-gcc &> /dev/null; then
    echo "Error: riscv64-linux-gnu-gcc not found" >&2
    exit 1
fi

# Same pin every other userspace object uses. -ffreestanding: the routines
# must not synthesize libc calls; -fno-stack-protector: freestanding target
# without __stack_chk_guard (musl static link discipline used everywhere).
CFLAGS="-march=rv64gc_zicsr -mabi=lp64d -O2 -ffreestanding -fno-stack-protector"

OBJS=()
for c in "$SRC"/*.c; do
    obj="${c%.c}.o"
    # clzdi2.c: -fno-builtin so gcc cannot fold the binary search back
    # into a __builtin_clz call (self-recursion).
    extra=""
    case "$c" in */clzdi2.c) extra="-fno-builtin";; esac
    riscv64-linux-gnu-gcc $CFLAGS $extra -I"$SRC" -c "$c" -o "$obj"
    OBJS+=("$obj")
done

rm -f "$OUT_A"
riscv64-linux-gnu-ar rcs "$OUT_A" "${OBJS[@]}"

# Gate: the overlay itself must stay pin-clean. Any z*b/zcb attribute here
# means the compiler stopped honoring -march and the whole scheme is void.
if riscv64-linux-gnu-readelf -A "$OUT_A" | grep -qE "zbb1p0|zba1p0|zbs1p0|zcb1p0|zicond1p0"; then
    echo "Error: libsoftfp-pin.a carries bitmanip/compressed-bitmanip attributes:" >&2
    riscv64-linux-gnu-readelf -A "$OUT_A" | grep -oE "zbb1p0|zba1p0|zbs1p0|zcb1p0|zicond1p0" | sort -u >&2
    exit 1
fi

echo "libsoftfp-pin.a built ($(ls -la "$OUT_A" | awk '{print $5}') bytes)"
