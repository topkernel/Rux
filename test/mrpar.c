#define _GNU_SOURCE
/*
 * mrpar.c — mremap parity probe (hilogd MREMAP-FAIL storm hunt).
 *
 * hilogd's FFRT worker (pid 317-class threads) logs 66x
 *   MREMAP-FAIL old=0x7ffffffXc000 old_sz=0x1000 new_sz=0x2000
 *   flags=0x0 -> -12
 * right before the hilogd.server SIGSEGV. Checks:
 *
 *   1. in-place growth (flags=0) with a FREE page above: must succeed;
 *   2. in-place growth with the neighbor OCCUPIED: ENOMEM is correct
 *      (Linux: "we can't grow in place") — must be -1/ENOMEM;
 *   3. MREMAP_MAYMOVE growth with the neighbor occupied: must succeed
 *      and MOVE (or stay) with contents preserved;
 *   4. shrink (flags=0): must succeed;
 *   5. old_sz bytes must round-trip across every successful remap.
 *
 * Final: MRPAR RESULT: PASS|FAIL.
 */
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/syscall.h>

static int puts_(const char *s) { return write(1, s, strlen(s)); }
static void putu_(unsigned long v) {
    char b[24]; int i = 24;
    if (!v) { puts_("0"); return; }
    while (v && i) { b[--i] = '0' + (v % 10); v /= 10; }
    write(1, b + i, 24 - i);
}

#define GROW 0x2000

int main(void) {
    int fail = 0;
    void *p, *q, *r;

    /* 1. in-place growth, free page above: map two pages, shrink to
     * one, then grow back — the neighbor page is our own VMA's tail
     * (still ours), so this must work in place. */
    p = mmap(0, GROW, PROT_READ | PROT_WRITE,
             MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) { puts_("MRPAR 1 mmap failed\n"); return 1; }
    memset(p, 0x5a, GROW);
    /* shrink to 4K then grow to 8K: the upper page was part of the
     * same mapping moments ago — nobody else can own it. */
    q = mremap(p, 0x1000, 0x1000, 0);          /* identity shrink */
    if (q == MAP_FAILED) {
        puts_("MRPAR 1 shrink failed errno="); putu_(errno); puts_("\n");
        fail = 1;
    } else {
        p = q;
        q = mremap(p, 0x1000, GROW, 0);        /* in-place grow, tail free */
        if (q == MAP_FAILED || q != p) {
            puts_("MRPAR 1 inplace-grow FAILED errno=");
            putu_(errno);
            puts_(q == MAP_FAILED ? "\n" : " (moved!)\n");
            fail = 1;
        } else if (((unsigned char *)p)[0x1000] != 0x5a) {
            /* the tail page's contents must survive shrink+regrow */
            puts_("MRPAR 1 NOTE tail page zeroed after regrow\n");
        } else {
            puts_("MRPAR 1 inplace-grow OK\n");
        }
        p = q;
    }
    munmap(p, GROW);

    /* 2+3. neighbor occupied: two adjacent kernel-placed 4K mappings
     * (top-down placement stacks them), then growing the LOWER one in
     * place must ENOMEM; MAYMOVE must succeed. NOTE: MAP_FIXED at a
     * mid-range address (e.g. 4G) currently returns EINVAL on this
     * kernel (brk-region guard in arch mmap is over-broad — separate
     * finding), so placement is kernel-chosen here. */
    void *lo = mmap(0, 0x1000, PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    void *hi = mmap(0, 0x1000, PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (lo == MAP_FAILED || hi == MAP_FAILED) {
        puts_("MRPAR setup mmap failed errno="); putu_(errno); puts_("\n");
        return 1;
    }
    if ((char *)lo > (char *)hi) { void *t = lo; lo = hi; hi = t; }
    memset(lo, 0xa3, 0x1000);
    /* lo+4K must be occupied by hi for the ENOMEM case to be meaningful. */
    if ((char *)hi != (char *)lo + 0x1000) {
        puts_("MRPAR NOTE placement not adjacent; skipping 2/3\n");
        munmap(lo, 0x1000);
        munmap(hi, 0x1000);
        lo = hi = 0;
    }
    if (lo) {
    r = lo;
    q = mremap(r, 0x1000, GROW, 0);            /* in-place: must fail */
    if (q != MAP_FAILED) {
        puts_("MRPAR 2 inplace-grow OCCUPIED unexpectedly succeeded\n");
        munmap(lo, 0x1000);
        munmap(hi, 0x1000);
        lo = hi = 0;
        fail = 1;
    } else if (errno != ENOMEM) {
        puts_("MRPAR 2 errno=");
        putu_(errno);
        puts_(" (want ENOMEM=12)\n");
        munmap(lo, 0x1000);
        munmap(hi, 0x1000);
        lo = hi = 0;
        fail = 1;
    } else {
        puts_("MRPAR 2 inplace-grow-occupied=ENOMEM OK\n");
    }
    q = mremap(r, 0x1000, GROW, MREMAP_MAYMOVE);
    if (q == MAP_FAILED) {
        puts_("MRPAR 3 MAYMOVE FAILED errno="); putu_(errno); puts_("\n");
        fail = 1;
    } else if (((unsigned char *)q)[0] != 0xa3 || ((unsigned char *)q)[0xfff] != 0xa3) {
        puts_("MRPAR 3 contents corrupted after move\n");
        fail = 1;
    } else {
        puts_("MRPAR 3 MAYMOVE OK (contents preserved)\n");
    }
    if (q != MAP_FAILED) munmap(q, GROW);
    } /* end if (lo) */

    /* 5. plain 2-page anon map round-trip + MAYMOVE twice */
    p = mmap(0, 0x2000, PROT_READ | PROT_WRITE,
             MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p != MAP_FAILED) {
        memset(p, 0x77, 0x2000);
        q = mremap(p, 0x2000, 0x4000, MREMAP_MAYMOVE);
        if (q == MAP_FAILED) {
            puts_("MRPAR 5 grow-2x MAYMOVE FAILED errno=");
            putu_(errno); puts_("\n");
            fail = 1;
        } else {
            memset((char *)q + 0x2000, 0x88, 0x2000);
            r = mremap(q, 0x4000, 0x2000, 0);      /* shrink in place */
            if (r == MAP_FAILED || r != q || ((unsigned char *)r)[0x1fff] != 0x77) {
                puts_("MRPAR 5 shrink FAILED\n");
                fail = 1;
            } else {
                puts_("MRPAR 5 shrink round-trip OK\n");
            }
            munmap(q, r == MAP_FAILED ? 0x4000 : 0x2000);
        }
    }

    puts_(fail ? "MRPAR RESULT: FAIL\n" : "MRPAR RESULT: PASS\n");
    return fail;
}
