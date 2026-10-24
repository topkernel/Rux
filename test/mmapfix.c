// mmapfix.c — MAP_FIXED low-placement + brk interplay probe (wildwrite
// lane session 15). Documents and validates the narrowed brk-conflict
// guard in the x86_64/riscv64 mmap MAP_FIXED path.
//
// Divergence under test (session-14 side-observation): the arch mmap
// guard `addr < MMAP_START && end > BRK_DEFAULT` rejected EVERY fixed
// mapping between 0x20000000 and MMAP_START as "brk conflict" — Linux
// allows any fixed placement in the user range (it unmaps whatever is
// covered). The fix refuses only the LIVE brk heap span
// [align_down(start_brk), align(brk)) and teaches brk growth to stop
// at foreign VMAs (Linux sys_brk: newbrk + PAGE must not reach the
// next VMA).
//
//   MF1  MAP_FIXED anon at 0x50000000: succeeds, content sticks.
//   MF2  MAP_FIXED anon at 0x30000000 (just above BRK_DEFAULT, far
//        from any live heap): succeeds.
//   MF3  MAP_FIXED straddling BRK_DEFAULT from below (0x1fff0000 +
//        0x20000): succeeds when nothing live is covered.
//   MF4  brk growth refusal: grow brk 8 pages, MAP_FIXED just above
//        the heap, then ask brk to grow INTO it — refused, brk
//        unchanged (Linux parity; was: silent growth underneath).
//   MF5  brk still grows UP TO one page below the fixed mapping
//        (leave-one-page rule), data writable at the new top.
//   MF5b brk to EXACTLY the neighbor's start — and to 16 bytes shy of
//        it — returns the OLD break (Linux sys_brk
//        find_vma_intersection(oldbrk, newbrk + PAGE); was: growth to
//        exact touch silently succeeded).
//   MF6  MAP_FIXED over the LIVE heap still fails EINVAL (deliberate
//        divergence: never unmap malloc's own storage).
//   MF7  MAP_FIXED_NOREPLACE over an existing mapping fails EEXIST.
//   MF8  fork COW over the MF1 fixed mapping: child's write is not
//        visible in the parent.
//   MF9  kernel-chosen mmap unaffected (NULL hint).
//   MF10 NON-FIXED low hint is HONORED: mmap(0x60000000) (no
//        MAP_FIXED) places exactly there (Linux arch_get_unmapped_area;
//        was: every hint below MMAP_START discarded to the high search
//        area).
//   MF11 unaligned non-fixed hint rounds DOWN (Linux addr &= PAGE_MASK;
//        was: unaligned hints reached the kernel VMA builder and hit
//        its page-alignment assert — a kernel panic vector).
//   MF12 hint below MIN_MAP_ADDR is IGNORED (kernel-chosen placement,
//        not an error — Linux mmap_min_addr behavior).
//   MF13 non-fixed hint onto the LIVE heap is IGNORED (kernel-chosen
//        placement; heap stays writable).
//
// PASS: prints MMF-PASS. Any failure prints MMF-FAIL with the leg name.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

static unsigned long raw_brk(unsigned long want)
{
    return (unsigned long)syscall(SYS_brk, want);
}

static void dump_maps(const char *tag)
{
    int mfd = open("/proc/self/maps", O_RDONLY);
    if (mfd < 0) {
        printf("maps unreadable errno=%d (%s)\n", errno, tag);
        return;
    }
    char buf[4096];
    ssize_t n;
    printf("--- maps %s ---\n", tag);
    while ((n = read(mfd, buf, sizeof(buf))) > 0)
        write(1, buf, n);
    printf("--- end maps %s ---\n", tag);
    close(mfd);
}

#define PAGE 4096UL

static int failures;

static void fail(const char *leg, const char *fmt, ...)
{
    printf("MMF-FAIL: %s: ", leg);
    va_list ap;
    va_start(ap, fmt);
    vprintf(fmt, ap);
    va_end(ap);
    printf("\n");
    failures++;
}

static void *xfixed(void *addr, size_t len)
{
    return mmap(addr, len, PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
}

int main(void)
{
    printf("mmapfix: pid=%d brk0=%p\n", getpid(), sbrk(0));

    /* Diagnostic: the image/heap-adjacent VMA layout (helps attribute
     * brk-growth refusals to a specific neighbor VMA). */
    {
        int mfd = open("/proc/self/maps", O_RDONLY);
        if (mfd >= 0) {
            char buf[4096];
            ssize_t n;
            printf("--- maps ---\n");
            while ((n = read(mfd, buf, sizeof(buf))) > 0)
                write(1, buf, n);
            printf("--- end maps ---\n");
            close(mfd);
        } else {
            printf("maps unreadable errno=%d\n", errno);
        }
    }

    /* MF1: fixed placement at a mid-low address Linux allows. */
    unsigned char *p1 = xfixed((void *)0x50000000UL, 0x10000);
    if (p1 == MAP_FAILED) {
        fail("MF1", "MAP_FIXED 0x50000000 errno=%d (Linux allows)", errno);
    } else {
        memset(p1, 0x5a, 0x10000);
        if (p1[0x8000] != 0x5a || p1[0xfff0] != 0x5a)
            fail("MF1", "content wrong after write");
        printf("MF1 ok: fixed 0x50000000 mapped, content sticks\n");
    }

    /* MF2: just above BRK_DEFAULT with nothing live. */
    if (xfixed((void *)0x30000000UL, 0x4000) == MAP_FAILED)
        fail("MF2", "MAP_FIXED 0x30000000 errno=%d", errno);
    else
        printf("MF2 ok: fixed 0x30000000 mapped\n");

    /* MF3: straddles BRK_DEFAULT from below. */
    if (xfixed((void *)0x1fff0000UL, 0x20000) == MAP_FAILED)
        fail("MF3", "MAP_FIXED 0x1fff0000 errno=%d", errno);
    else
        printf("MF3 ok: fixed straddling BRK_DEFAULT mapped\n");

    /* MF4/MF5: brk interplay. Growth first (also proves the plain brk
     * path still works), then a fixed mapping above the heap, then
     * refused growth into it and allowed growth up to one page below.
     * All brk manipulation below uses RAW syscalls: musl's sbrk is a
     * stub (src/linux/sbrk.c returns -ENOMEM for any nonzero inc —
     * musl malloc is mmap-only), so the libc wrapper never reaches the
     * kernel. */
    unsigned long b0 = (unsigned long)sbrk(0);
    if (sbrk(0) == (void *)-1)
        fail("MF4", "sbrk(0) failed");
    if (raw_brk(b0 + 8 * PAGE) != b0 + 8 * PAGE)
        fail("MF4", "baseline brk growth failed (pre-existing path)");
    unsigned long b1 = raw_brk(0);
    if (b1 != b0 + 8 * PAGE)
        fail("MF4", "brk did not advance: %lx -> %lx", b0, b1);
    else
        printf("MF4 ok: baseline brk growth %lx -> %lx\n", b0, b1);

    unsigned long fixed4 = ((b1 + 0x100000 + PAGE - 1) & ~(PAGE - 1));
    if (xfixed((void *)fixed4, 4 * PAGE) == MAP_FAILED) {
        fail("MF4", "MAP_FIXED above heap @%#lx errno=%d", fixed4, errno);
    } else {
        /* Ask brk to grow INTO the fixed mapping: must be refused,
         * returning the old break (Linux sys_brk shape). */
        unsigned long prev = raw_brk(fixed4 + 2 * PAGE);
        unsigned long after = raw_brk(0);
        if (prev != b1 || after != b1)
            fail("MF4", "growth into fixed mapping not refused: ret=%lx "
                        "after=%lx (want %lx)", prev, after, b1);
        else
            printf("MF4 ok: growth into fixed mapping refused, brk=%lx\n",
                   after);

        /* MF5: growth up to one page BELOW the mapping must succeed. */
        unsigned long target = fixed4 - PAGE;
        if (raw_brk(target) != target) {
            fail("MF5", "brk growth below fixed mapping failed (%lx)",
                 raw_brk(0));
        } else {
            unsigned long b2 = raw_brk(0);
            if (b2 != target)
                fail("MF5", "brk stopped at %lx, expected %lx", b2, target);
            else {
                *(volatile unsigned char *)(b2 - 8) = 0xc3;
                if (*(volatile unsigned char *)(b2 - 8) != 0xc3)
                    fail("MF5", "new brk top not writable");
                else
                    printf("MF5 ok: brk grew to one page below mapping\n");
            }
        }

        /* MF5b: the one-page gap is enforced at the BREAK level too —
         * growth to EXACTLY the neighbor's start, and to 16 bytes shy
         * of it (which still claims the touching page), both return
         * the OLD break (Linux sys_brk: any VMA overlapping
         * [oldbrk, newbrk + PAGE) refuses the growth). */
        {
            unsigned long prev = raw_brk(fixed4);
            if (prev != target || raw_brk(0) != target)
                fail("MF5b", "brk to exact neighbor start not refused: "
                            "ret=%lx now=%lx (want %lx)", prev, raw_brk(0),
                     target);
            else {
                prev = raw_brk(fixed4 - PAGE + 16);
                if (prev != target || raw_brk(0) != target)
                    fail("MF5b", "brk to 16-bytes-shy not refused: ret=%lx "
                                "now=%lx (want %lx)", prev, raw_brk(0),
                         target);
                else
                    printf("MF5b ok: exact-touch and 16-bytes-shy brk "
                           "refused, brk=%lx\n", target);
            }
        }
    }

    /* MF6: MAP_FIXED over the LIVE heap stays EINVAL (deliberate
     * divergence — the heap's storage is never silently unmapped).
     * Live heap is [align_down(start_brk), align(brk)); target its
     * last page. */
    unsigned long bcur = raw_brk(0);
    unsigned long heap_mid = ((bcur + PAGE - 1) & ~(PAGE - 1)) - PAGE;
    errno = 0;
    void *r6 = mmap((void *)heap_mid, PAGE, PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
    if (r6 != MAP_FAILED || errno != EINVAL)
        fail("MF6", "live-heap MAP_FIXED not EINVAL: r=%p errno=%d", r6,
             errno);
    else
        printf("MF6 ok: live-heap MAP_FIXED refused EINVAL @%lx\n",
               heap_mid);

    /* MF7: MAP_FIXED_NOREPLACE over the MF1 mapping -> EEXIST. */
    if (p1 != MAP_FAILED) {
        errno = 0;
        void *r7 = mmap(p1, PAGE, PROT_READ,
                        MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1,
                        0);
        if (r7 != MAP_FAILED || errno != EEXIST)
            fail("MF7", "NOREPLACE over mapping not EEXIST: r=%p errno=%d",
                 r7, errno);
        else
            printf("MF7 ok: NOREPLACE EEXIST\n");

        /* MF8: fork COW over the fixed mapping. Raw-write diagnostics
         * from the child (printf from short-lived children is lossy
         * on this console). */
        pid_t pid = fork();
        if (pid == 0) {
            p1[0x4000] = 0x77;
            static const char msg[] = "MF8 child wrote\n";
            write(1, msg, sizeof(msg) - 1);
            _exit(0);
        }
        int st = 0;
        waitpid(pid, &st, 0);
        if (!WIFEXITED(st) || WEXITSTATUS(st) != 0)
            fail("MF8", "child died st=%d", st);
        else if (p1[0x4000] != 0x5a)
            fail("MF8", "child write leaked into parent (COW broken)");
        else
            printf("MF8 ok: fork COW isolates the fixed mapping\n");
    }

    /* MF9: kernel-chosen placement unaffected. */
    void *r9 = mmap(NULL, 0x20000, PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (r9 == MAP_FAILED)
        fail("MF9", "NULL-hint mmap errno=%d", errno);
    else {
        memset(r9, 1, 0x20000);
        printf("MF9 ok: NULL-hint mmap at %p\n", r9);
    }

    /* MF10: a NON-FIXED low hint in a free gap is honored verbatim
     * (Linux arch_get_unmapped_area honors free hints anywhere in the
     * user range; was: hints below MMAP_START were discarded to the
     * high kernel-chosen area). 0x60000000 is clear of MF1-MF4's
     * placements on both arches. */
    void *r10 = mmap((void *)0x60000000UL, 0x8000, PROT_READ | PROT_WRITE,
                     MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (r10 == MAP_FAILED)
        fail("MF10", "low-hint mmap errno=%d", errno);
    else if ((unsigned long)r10 != 0x60000000UL)
        fail("MF10", "low hint not honored: got %p (want 0x60000000)", r10);
    else {
        memset(r10, 0x33, 0x8000);
        if (((unsigned char *)r10)[0x7ff0] != 0x33)
            fail("MF10", "hinted mapping content wrong");
        else
            printf("MF10 ok: low hint honored at %p\n", r10);
    }

    /* MF11: unaligned non-fixed hint rounds DOWN (Linux
     * `addr &= PAGE_MASK`). Was: the raw hint reached the kernel VMA
     * builder and hit its page-alignment assert — kernel panic. */
    void *r11 = mmap((void *)0x60010001UL, 0x4000, PROT_READ | PROT_WRITE,
                     MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (r11 == MAP_FAILED)
        fail("MF11", "unaligned-hint mmap errno=%d", errno);
    else if ((unsigned long)r11 != 0x60010000UL)
        fail("MF11", "unaligned hint not rounded down: got %p "
                     "(want 0x60010000)", r11);
    else
        printf("MF11 ok: unaligned hint rounded down to %p\n", r11);

    /* MF12: hint below MIN_MAP_ADDR (4KB null-page guard) is IGNORED —
     * success at a kernel-chosen address, never the hint itself. */
    void *r12 = mmap((void *)0x800UL, PAGE, PROT_READ | PROT_WRITE,
                     MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (r12 == MAP_FAILED)
        fail("MF12", "sub-MIN_MAP_ADDR hint mmap errno=%d (must succeed "
                     "with kernel placement)", errno);
    else if ((unsigned long)r12 == 0x800UL)
        fail("MF12", "sub-MIN_MAP_ADDR hint must be ignored, got 0x800");
    else
        printf("MF12 ok: sub-MIN_MAP_ADDR hint ignored, placed at %p\n",
               r12);

    /* MF13: non-fixed hint onto the LIVE heap is IGNORED (kernel
     * placement; the heap keeps working). Live heap's last full page. */
    {
        unsigned long bcur13 = raw_brk(0);
        unsigned long heap_last = ((bcur13 + PAGE - 1) & ~(PAGE - 1)) - PAGE;
        void *r13 = mmap((void *)heap_last, PAGE, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (r13 == MAP_FAILED)
            fail("MF13", "live-heap hint mmap errno=%d (must succeed with "
                         "kernel placement)", errno);
        else if ((unsigned long)r13 == heap_last)
            fail("MF13", "hint onto live heap must be ignored");
        else {
            *(volatile unsigned char *)(bcur13 - 8) = 0x31;
            if (*(volatile unsigned char *)(bcur13 - 8) != 0x31)
                fail("MF13", "heap top not writable after hint fallback");
            else
                printf("MF13 ok: live-heap hint ignored, placed at %p, "
                       "heap intact\n", r13);
        }
    }

    if (failures) {
        printf("MMF-FAIL: %d leg(s)\n", failures);
        return 1;
    }
    printf("MMF-PASS\n");
    return 0;
}
