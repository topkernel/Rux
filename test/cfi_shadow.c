// cfi_shadow.c — replicate OH musl's Cross-DSO CFI shadow init EXACTLY.
//
// Disassembled from the shipped /lib/ld-musl-x86_64.so.1 (system.img):
//   init_cfi_shadow / map_dso_to_cfi_shadow:
//     reserve = mmap(NULL, 0x80000000, PROT_NONE,
//                    MAP_PRIVATE|MAP_ANONYMOUS|MAP_NORESERVE)
//   fill_range(begin, end, value, mode) [sub_89d80]:
//     first   = reserve + (begin >> 18) * 2      // 2-byte entry per 256KB
//     last    = reserve + ((end-1) >> 18) * 2
//     s_start = first & ~0xfff
//     s_end   = (last + 0x1001) & ~0xfff
//     len     = (u16)(u32)(s_end - s_start)      // 16-bit truncated length
//     tmp     = mmap(NULL, len, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANON)
//     mprotect(s_start, len, PROT_READ)          // over the PROT_NONE reserve
//     memcpy(tmp, s_start, len)                  // preserve neighbor entries
//     write u16 entries into tmp (value + 0x40*k, >0xffff -> 1)
//     mprotect(tmp, len, PROT_READ)
//     mremap(tmp, len, len, MREMAP_MAYMOVE|MREMAP_FIXED, s_start)
//       [on failure: munmap(tmp), slice stays as-is]
//
// Every cross-DSO indirect call then reads
//   *(u16 *)(reserve + (target >> 18) * 2)
// so a slice whose fill failed leaves a PROT_NONE read -> SIGSEGV at
// rip inside __cfi_slowpath (the OH "Permission denied at 0x3fbf089ac6"
// family that kills samgr's FFRT workers).
//
// Failure criteria (any = CFI-FAIL):
//   F1  any syscall in the fill sequence returns an error
//   F2  mremap returns success but a wrong address
//   F3  an entry read of a FILLED slice faults or reads the wrong value
//       (data lost by the move / clobbered by the MAP_FIXED replacement)
//   F4  neighbor entries in a shared slice page are lost by a later fill
//       (the memcpy-preserve path depends on mremap'd data being readable)
//   F5  the moved range does NOT carry the source protections (PROT_READ
//       at mremap time: a write must fault, a read must not)
//   F6  the old tmp mapping survives the move
//   F7  a reader thread faults while another thread re-fills the same
//       slice (Linux holds mmap_lock across mremap: the range is
//       continuously mapped, never a PROT_NONE hole)
//
// PASS: prints CFI-PASS.
#define _GNU_SOURCE
#include <errno.h>
#include <setjmp.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <pthread.h>
#include <unistd.h>

#define RESERVE_SZ 0x80000000UL   /* 2GB, like the PROTNONE-MAP len 0x80000000 */
#define PAGE       4096UL

static unsigned long reserve;
static int failures;
static const char *phase;

/* ---- expected-SIGSEGV machinery ---- */
static __thread sigjmp_buf fault_jmp;
static __thread volatile sig_atomic_t fault_ok;
static void fault_handler(int sig, siginfo_t *si, void *uc)
{
    (void)sig; (void)si; (void)uc;
    if (fault_ok)
        siglongjmp(fault_jmp, 1);
    /* unexpected fault */
    fprintf(stderr, "CFI-FAIL: unexpected SIGSEGV addr=%p in phase '%s'\n",
            si->si_addr, phase);
    _exit(3);
}

/* returns 1 if reading *p faulted */
static int read_faults(volatile uint16_t *p)
{
    fault_ok = 1;
    if (sigsetjmp(fault_jmp, 1) == 0) {
        (void)*p;
        fault_ok = 0;
        return 0;
    }
    fault_ok = 0;
    return 1;
}
static int write_faults(volatile uint16_t *p)
{
    fault_ok = 1;
    if (sigsetjmp(fault_jmp, 1) == 0) {
        *p = 0xdead;
        fault_ok = 0;
        return 0;
    }
    fault_ok = 0;
    return 1;
}

static void fail(const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    fprintf(stderr, "CFI-FAIL(%s): ", phase);
    vfprintf(stderr, fmt, ap);
    fprintf(stderr, "\n");
    va_end(ap);
    failures++;
}

#define STEP(call, name) do {                          \
    if ((call) != 0) {                                  \
        int _e = errno;                                 \
        fail("%s errno=%d (%s)", name, _e, strerror(_e));\
        return -1;                                      \
    }                                                   \
} while (0)

/* musl's fill_range, mode 2 (write value+0x40*k entries).
 * skip_tmp_check: concurrent fillers can get the just-freed tmp address
 * from mmap, making the "old tmp is gone" probe racy — only meaningful
 * single-threaded. */
static int cfi_fill(unsigned long begin, unsigned long end, unsigned value,
                    int skip_tmp_check)
{
    unsigned long first = reserve + ((begin >> 18) * 2);
    unsigned long last  = reserve + (((end - 1) >> 18) * 2);
    unsigned long s_start = first & ~(PAGE - 1);
    unsigned long s_end   = (last + PAGE + 1) & ~(PAGE - 1);
    unsigned len = (unsigned)(uint16_t)(uint32_t)(s_end - s_start);

    void *tmp = mmap(NULL, len, PROT_READ | PROT_WRITE,
                     MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (tmp == MAP_FAILED) {
        fail("tmp mmap len=%u errno=%d", len, errno);
        return -1;
    }
    STEP(mprotect((void *)s_start, len, PROT_READ), "mprotect(slice, PROT_READ)");
    memcpy(tmp, (void *)s_start, len);      /* F4: must not fault, preserves neighbors */
    /* write the entries (value + 0x40*k, cap at >0xffff -> 1, like musl) */
    unsigned long e = first;
    unsigned v = value;
    for (; e <= last; e += 2, v += 0x40) {
        unsigned short en = (v > 0xffff) ? 1 : (unsigned short)v;
        /* write via tmp at the same offset the entry will live at */
        unsigned long off = e - s_start;
        memcpy((char *)tmp + off, &en, 2);
    }
    STEP(mprotect(tmp, len, PROT_READ), "mprotect(tmp, PROT_READ)");
    void *res = mremap(tmp, len, len, MREMAP_MAYMOVE | MREMAP_FIXED,
                       (void *)s_start);
    if (res == MAP_FAILED) {
        int _e = errno;
        munmap(tmp, len);
        fail("mremap(MREMAP_FIXED) len=%u errno=%d (%s)", len, _e, strerror(_e));
        return -1;
    }
    if (res != (void *)s_start) {
        fail("mremap returned %p != slice %p", res, (void *)s_start);
        return -1;
    }
    /* F6: the old tmp mapping must be gone */
    if (!skip_tmp_check && !read_faults((volatile uint16_t *)tmp)) {
        fail("old tmp mapping still readable after mremap");
        return -1;
    }
    return 0;
}

/* expected entry for target t (mirrors the writer above) */
static unsigned short expect_entry(unsigned long begin, unsigned value,
                                   unsigned long t)
{
    unsigned k = (unsigned)((t >> 18) - (begin >> 18));
    unsigned v = value + 0x40 * k;
    return (unsigned short)(v > 0xffff ? 1 : v);
}

static volatile uint16_t entry_at(unsigned long target)
{
    return *(volatile uint16_t *)(reserve + ((target >> 18) * 2));
}

/* ---------- P1: single slice, main-exe shape (page 0 of reserve) ---- */
static void p1_basic(void)
{
    phase = "P1";
    unsigned long begin = 0x60000, end = 0x260000;   /* fake main exe span */
    unsigned value = 0x1234;

    if (read_faults((volatile uint16_t *)(reserve + 4)) == 0)
        fail("reserve head readable before any fill");

    if (cfi_fill(begin, end, value, 0) != 0)
        return;

    /* F3: every entry readable + correct */
    for (unsigned long t = begin; t < end; t += 0x40000) {
        volatile uint16_t got = entry_at(t);
        if (got != expect_entry(begin, value, t)) {
            fail("entry @target %#lx = %#x want %#x", t, got,
                 expect_entry(begin, value, t));
            return;
        }
    }
    /* F5: slice is PROT_READ (source protections carried) */
    if (!write_faults((volatile uint16_t *)(reserve + 4)))
        fail("write to moved PROT_READ slice did NOT fault");
    if (read_faults((volatile uint16_t *)(reserve + 4)))
        fail("read of moved slice faulted");
    /* untouched page deep in the reserve must still fault */
    if (!read_faults((volatile uint16_t *)(reserve + 0x100000)))
        fail("unfilled reserve page readable");
    printf("P1 basic fill OK\n");
}

/* ---------- P2: shared slice page, neighbor preservation ---------- */
static void p2_shared_page(void)
{
    phase = "P2";
    /* two modules whose entries land in the SAME shadow page:
     * targets 0x3fbf000000 and 0x3fbf400000 -> entries 0x1fc792/2... —
     * pick targets 256KB apart so entries are 2 bytes apart in one page */
    unsigned long b1 = 0x3fbf000000UL, e1 = b1 + 0x40000;
    unsigned long b2 = b1 + 0x40000,     e2 = b1 + 0x80000;

    if (cfi_fill(b1, e1, 0x1111, 0) != 0) return;
    if (cfi_fill(b2, e2, 0x2222, 0) != 0) return;

    unsigned short a = entry_at(b1), b = entry_at(b2);
    if (a != expect_entry(b1, 0x1111, b1))
        fail("neighbor entry lost: %#x want %#x", a, expect_entry(b1, 0x1111, b1));
    if (b != expect_entry(b2, 0x2222, b2))
        fail("second entry wrong: %#x want %#x", b, expect_entry(b2, 0x2222, b2));
    printf("P2 shared-page preserve OK\n");
}

/* ---------- P3: re-fill same slice ------------------------------- */
static void p3_refill(void)
{
    phase = "P3";
    unsigned long b = 0x200000000UL, e = b + 0x80000;
    if (cfi_fill(b, e, 0x3333, 0) != 0) return;
    if (entry_at(b) != expect_entry(b, 0x3333, b)) { fail("first fill entry"); return; }
    if (cfi_fill(b, e, 0x4444, 0) != 0) return;
    if (entry_at(b) != expect_entry(b, 0x4444, b))
        fail("re-fill did not replace entry: %#x", entry_at(b));
    else
        printf("P3 re-fill OK\n");
}

/* ---------- P4/P7: concurrent fills + readers --------------------- */
#define NREAD 3
static volatile int stop_flag;
static volatile long fill_gen;
static unsigned long pb = 0x400000000UL, pe = 0x400000000UL + 0x1000000UL;

static void *filler(void *arg)
{
    (void)arg;
    unsigned v = 0x5000;
    while (!stop_flag) {
        for (unsigned long b = pb; b < pe; b += 0x100000) {
            if (cfi_fill(b, b + 0x80000, v, 1) != 0) {
                stop_flag = 1;
                return NULL;
            }
        }
        v += 0x111;
        __sync_fetch_and_add(&fill_gen, 1);
    }
    return NULL;
}

static void *reader(void *arg)
{
    (void)arg;
    long last_gen = -1;
    while (!stop_flag) {
        long g = fill_gen;
        if (g == 0) { usleep(200); continue; }
        if (g != last_gen) {
            /* sweep every entry of the whole filled window: all slices
             * were filled at least once by gen>=1, so NO read may fault
             * (F7) and the values must match SOME generation's pattern */
            for (unsigned long t = pb; t < pe; t += 0x40000) {
                if (read_faults((volatile uint16_t *)(reserve + ((t >> 18) * 2)))) {
                    fail("reader fault @target %#lx (gen %ld)", t, g);
                    stop_flag = 1;
                    return NULL;
                }
            }
            last_gen = g;
        }
    }
    return NULL;
}

static void p4_threaded(void)
{
    phase = "P4";
    pthread_t ft[2], rt[NREAD];
    stop_flag = 0; fill_gen = 0;
    for (int i = 0; i < 2; i++) pthread_create(&ft[i], NULL, filler, NULL);
    for (int i = 0; i < NREAD; i++) pthread_create(&rt[i], NULL, reader, NULL);
    sleep(4);
    stop_flag = 1;
    for (int i = 0; i < 2; i++) pthread_join(ft[i], NULL);
    for (int i = 0; i < NREAD; i++) pthread_join(rt[i], NULL);
    if (failures == 0)
        printf("P4 threaded churn OK (gen=%ld)\n", fill_gen);
}

int main(void)
{
    struct sigaction sa = {0};
    sa.sa_sigaction = fault_handler;
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGSEGV, &sa, NULL);
    sigaction(SIGBUS, &sa, NULL);

    setvbuf(stdout, NULL, _IONBF, 0);
    setvbuf(stderr, NULL, _IONBF, 0);

    phase = "init";
    void *r = mmap(NULL, RESERVE_SZ, PROT_NONE,
                   MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
    if (r == MAP_FAILED) {
        printf("CFI-FAIL: 2GB PROT_NONE reserve errno=%d\n", errno);
        return 1;
    }
    reserve = (unsigned long)r;
    printf("reserve at %#lx len %#lx\n", reserve, RESERVE_SZ);

    p1_basic();
    p2_shared_page();
    p3_refill();
    p4_threaded();

    if (failures) {
        printf("CFI-FAIL: %d failures\n", failures);
        return 1;
    }
    printf("CFI-PASS\n");
    return 0;
}
