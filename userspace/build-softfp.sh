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

# Gate: the overlay itself must stay pin-clean. Any attribute beyond the
# pinned march means the compiler stopped honoring -march (or the host gcc
# default marched on again — gcc-15.2 defaults to rv64...v_zcb_zfa_zicbo*)
# and the whole scheme is void. The pattern covers every extension that
# encodes instructions illegal on the project's stock QEMU cpu
# (-cpu rv64,zbb=true,zba=true,zbs=true); see the matching gate in
# mrsh/build-mrsh.sh.
BAD_ATTRS='zbb1p0|zba1p0|zbs1p0|zcb1p0|zicond1p0|zcmop1p0|zimop1p0|zfa1p0|zawrs1p0|zicbo[mp]1p0|zicboz1p0|_v1p0|zve(32|64)[fdx]?1p0|zvbb1p0|zvkb1p0|zvl[0-9]+b1p0'
if riscv64-linux-gnu-readelf -A "$OUT_A" | grep -qE "$BAD_ATTRS"; then
    echo "Error: libsoftfp-pin.a carries attributes beyond the pinned march:" >&2
    riscv64-linux-gnu-readelf -A "$OUT_A" | grep -oE "$BAD_ATTRS" | sort -u >&2
    exit 1
fi

echo "libsoftfp-pin.a built ($(ls -la "$OUT_A" | awk '{print $5}') bytes)"

# ---------------------------------------------------------------------------
# Pin-clean libgcc.a shadow (gcc-pin/libgcc.a)
#
# LTP links with `$(CC) $(LDFLAGS) $^ $(LDLIBS)` (rules.mk), i.e. every user
# library sits BEFORE the objects; at that point nothing is undefined yet, so
# an overlay archive listed in LDFLAGS is skipped and the gcc driver's
# trailing -lgcc resolves __letf2/__getf2/__multf3/__trunctfdf2 (referenced by
# musl's libc.a fmodl.lo) from Ubuntu's libgcc.a, whose members carry
# zbs/zicond encodings the pinned QEMU CPU model cannot execute (SIGILL).
#
# Position cannot win this, so we win by SEARCH PATH: this script copies the
# host libgcc.a and swaps the quad-float members for the pin-clean objects
# built above. Links that pass `-B<soft-fp>/gcc-pin/` find this copy before
# the system one, so every -lgcc resolution is pin-clean regardless of order.
# Members that still contain bitmanip after the swap abort the build (gate
# below), so a future gcc upgrade that adds new zicond members cannot sneak
# through silently.
GCC_PIN_DIR="${SRC}/gcc-pin"
SYSTEM_LIBGCC=$(ls /usr/lib/gcc-cross/riscv64-linux-gnu/*/libgcc.a 2>/dev/null | sort -V | tail -1)
if [ -z "$SYSTEM_LIBGCC" ]; then
    echo "Warning: system riscv64 libgcc.a not found; gcc-pin shadow not built" >&2
    exit 0
fi
mkdir -p "$GCC_PIN_DIR"
cp "$SYSTEM_LIBGCC" "${GCC_PIN_DIR}/libgcc.a"

# Swap every quad-float member the overlay provides (archive member names
# match the .c basenames: letf2.o, getf2.o, multf3.o, trunctfdf2.o, ...).
SWAP_OBJS=()
for c in "$SRC"/*.c; do
    obj="${c%.c}.o"
    if riscv64-linux-gnu-ar t "$SYSTEM_LIBGCC" | grep -qx "${obj##*/}"; then
        SWAP_OBJS+=("$obj")
    fi
done
if [ ${#SWAP_OBJS[@]} -gt 0 ]; then
    riscv64-linux-gnu-ar r "${GCC_PIN_DIR}/libgcc.a" "${SWAP_OBJS[@]}"
fi

# Gate: every member we swapped must itself be pin-clean (proves the swap
# objects honor -march; catches a compiler that ignores the pin).
for obj in "${SWAP_OBJS[@]}"; do
    if riscv64-linux-gnu-objdump -d "$obj" 2>/dev/null | \
        grep -qE "czero\.(eqz|nez)|\bbseti?\b|\bandn\b|\brolw?\b|\brorw?\b|\bclz\b"; then
        echo "Error: swap object $obj contains bitmanip encodings" >&2
        exit 1
    fi
done

# Report (non-fatal) members that still carry offender encodings. They only
# ship when a link actually references their symbols; _Float16 conversions
# (fixhfdi) and ifunc feature probes (feature_bits) are unreferenced by LTP.
riscv64-linux-gnu-objdump -d "${GCC_PIN_DIR}/libgcc.a" 2>/dev/null | \
    grep -E "czero\.(eqz|nez)|\bbseti?\b" > /dev/null && \
    echo "Warning: gcc-pin shadow still has bitmanip sites in uns members (fixhfdi/feature_bits); unused by LTP links" >&2

echo "gcc-pin/libgcc.a shadow built at ${GCC_PIN_DIR} (${#SWAP_OBJS[@]} members swapped)"
