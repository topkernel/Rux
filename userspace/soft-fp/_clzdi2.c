/* Pin-clean replacement for Ubuntu libgcc's _clzdi2.o member (zcb
 * encodings — c.lbu table lookups — SIGILL on the pinned QEMU CPU).
 * Ubuntu prefixes these archive members with '_'; the file basename must
 * match the member name for the build-softfp.sh shadow swap. The
 * implementation lives in clzdi2.c (compiled with -fno-builtin there).
 */
#include "clzdi2.c"
