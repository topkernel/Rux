// vma_storm.c — VMA-tree race storm: MAP_FIXED segment overlay (dlopen
// pattern) racing pthread-style stack mmap/mprotect/munmap churn, fork
// COW churn, and brk churn, all in one multithreaded process.
//
// Kernel races under test (mm-audit, OH heap-corruption hunt):
//   R1. mmap placement TOCTOU: the address scan and the VMA insert must
//       be one atomic section per mm (Linux holds mmap_lock across both).
//       A losing thread must never observe EINVAL/EEXIST-style failure
//       for a plain anonymous mmap, and two mmaps must never hand out the
//       same range.
//   R2. MAP_FIXED keep-piece surgery: when MAP_FIXED splits an existing
//       VMA, the preserved head/tail pieces must be re-installed
//       atomically with the removal. A racing mmap that lands in the
//       window between "overlap removed" and "pieces re-added" must not
//       silently drop a piece (a dropped piece = live mapping with no
//       VMA -> random SIGSEGV / foreign mapping inside a reserve).
//   R3. mprotect(PROT_NONE -> RW) split correctness under concurrency:
//       the RW region must appear exactly once; stack canaries written
//       after mprotect must survive (no zeroing, no lost split).
//
// Topology:
//   - 1 "dlopen" thread: mmap a 16MB PROT_NONE reserve, overlay ~10 RW
//     segments at fixed offsets with MAP_FIXED, marker-verify them, then
//     munmap the whole reserve.  Repeat.
//   - N "pthread" threads: mmap 2MB PROT_NONE, mprotect all but a guard
//     page to RW, canary-fill + verify a spread of pages, munmap.
//     Repeat.  (Exactly what musl __pthread_create/__pthread_exit do.)
//   - 1 "fork" thread: fork children that touch COW pages and _exit.
//   - 1 "brk" thread: grow/shrink the data segment in a loop.
//
// PASS: runs for <seconds> (default 30), zero VMAFAIL lines, prints
// VMASTORM-PASS.  Any corruption prints VMAFAIL-... with addresses.
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <sys/syscall.h>

#define RESERVE_SIZE   (16UL << 20)
#define STACK_SIZE     (2UL << 20)
#define GUARD_SIZE     (1UL << 12)
#define SEG_COUNT      10
#define SEG_LEN        (128UL << 10)
#define SEG_STRIDE     (1UL << 20)
#define CANARY_BASE    0x5a17c0de0000ULL

static volatile int stop_flag;
static unsigned long fail_count;
static unsigned long dlopen_rounds, stack_rounds, fork_rounds, brk_rounds;

static void fail(const char *what, unsigned long a, unsigned long b)
{
    /* write(2)-based: printf may itself fault once the mm is suspect */
    char buf[160];
    int n = snprintf(buf, sizeof(buf), "VMAFAIL-%s a=%#lx b=%#lx\n", what, a, b);
    if (n > 0)
        (void)write(2, buf, n);
    __sync_fetch_and_add(&fail_count, 1);
}

static void sigsegv_handler(int sig, siginfo_t *si, void *ctx)
{
    (void)sig; (void)ctx;
    char buf[128];
    int n = snprintf(buf, sizeof(buf), "VMAFAIL-SIGSEGV addr=%p errno=%d\n",
                     si->si_addr, si->si_errno);
    if (n > 0)
        (void)write(2, buf, n);
    _exit(42);
}

/* page canary for (region, page index, thread salt) */
static inline unsigned long canary(unsigned long base, unsigned long page, unsigned long salt)
{
    return CANARY_BASE ^ (base & 0xfffffffff000UL) ^ (page * 2654435761UL) ^ (salt * 40503UL);
}

static void *dlopen_thread(void *arg)
{
    long id = (long)arg;
    unsigned long salt = 0x1000 + id;

    while (!stop_flag) {
        char *p = mmap(NULL, RESERVE_SIZE, PROT_NONE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (p == MAP_FAILED) {
            fail("dlopen-reserve-mmap", errno, 0);
            usleep(1000);
            continue;
        }

        for (int k = 0; k < SEG_COUNT; k++) {
            char *seg = p + (k + 1) * SEG_STRIDE; /* leave holes between segs */
            void *r = mmap(seg, SEG_LEN, PROT_READ | PROT_WRITE,
                           MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
            if (r != seg) {
                fail("dlopen-seg-mmap", (unsigned long)seg, errno);
                break;
            }
            unsigned long *w = (unsigned long *)seg;
            for (unsigned long i = 0; i < SEG_LEN / 8; i += 64)
                w[i] = canary((unsigned long)seg, i, salt);
        }

        /* verify every segment marker survived concurrent churn */
        for (int k = 0; k < SEG_COUNT; k++) {
            char *seg = p + (k + 1) * SEG_STRIDE;
            unsigned long *w = (unsigned long *)seg;
            for (unsigned long i = 0; i < SEG_LEN / 8; i += 64) {
                if (w[i] != canary((unsigned long)seg, i, salt)) {
                    fail("dlopen-marker", (unsigned long)&w[i], w[i]);
                    k = SEG_COUNT;
                    break;
                }
            }
        }

        if (munmap(p, RESERVE_SIZE) != 0)
            fail("dlopen-munmap", (unsigned long)p, errno);
        __sync_fetch_and_add(&dlopen_rounds, 1);
    }
    return NULL;
}

static void *stack_thread(void *arg)
{
    long id = (long)arg;
    unsigned long salt = 0x9000 + id;

    while (!stop_flag) {
        char *q = mmap(NULL, STACK_SIZE, PROT_NONE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (q == MAP_FAILED) {
            /* plain anonymous mmap failing (e.g. EINVAL from a placement
             * race) is itself a kernel bug — Linux only fails ENOMEM here */
            fail("stack-mmap", errno, 0);
            usleep(1000);
            continue;
        }
        if (mprotect(q + GUARD_SIZE, STACK_SIZE - GUARD_SIZE,
                     PROT_READ | PROT_WRITE) != 0) {
            fail("stack-mprotect", (unsigned long)q, errno);
            munmap(q, STACK_SIZE);
            continue;
        }

        unsigned long *w = (unsigned long *)(q + GUARD_SIZE);
        unsigned long npg = (STACK_SIZE - GUARD_SIZE) / 4096;
        for (unsigned long pg = 0; pg < npg; pg += 16) {
            unsigned long *page = w + pg * 512;
            page[0] = canary((unsigned long)q, pg, salt);
            page[511] = canary((unsigned long)q, pg, ~salt);
        }
        /* yield into the churn, then verify */
        sched_yield();
        for (unsigned long pg = 0; pg < npg; pg += 16) {
            unsigned long *page = w + pg * 512;
            if (page[0] != canary((unsigned long)q, pg, salt)) {
                fail("stack-canary-head", (unsigned long)&page[0], page[0]);
                break;
            }
            if (page[511] != canary((unsigned long)q, pg, ~salt)) {
                fail("stack-canary-tail", (unsigned long)&page[511], page[511]);
                break;
            }
        }

        if (munmap(q, STACK_SIZE) != 0)
            fail("stack-munmap", (unsigned long)q, errno);
        __sync_fetch_and_add(&stack_rounds, 1);
    }
    return NULL;
}

static void *fork_thread(void *arg)
{
    (void)arg;
    static char cow_page[4096] __attribute__((aligned(4096)));

    while (!stop_flag) {
        cow_page[100]++;
        pid_t pid = fork();
        if (pid < 0) {
            fail("fork", errno, 0);
            usleep(1000);
            continue;
        }
        if (pid == 0) {
            cow_page[200]++; /* force COW break in the child */
            _exit(0);
        }
        int st;
        while (waitpid(pid, &st, 0) < 0 && errno == EINTR)
            ;
        __sync_fetch_and_add(&fork_rounds, 1);
    }
    return NULL;
}

static void *brk_thread(void *arg)
{
    (void)arg;
    unsigned long salt = 0x3300;

    while (!stop_flag) {
        intptr_t base = (intptr_t)sbrk(0);
        if (base == (intptr_t)-1)
            break;
        if (sbrk(64 * 4096) == (void *)-1) {
            usleep(1000);
            continue;
        }
        char *b = (char *)sbrk(0) - 4096;
        *(unsigned long *)b = canary((unsigned long)base, 1, salt);
        if (*(unsigned long *)b != canary((unsigned long)base, 1, salt))
            fail("brk-canary", (unsigned long)b, *(unsigned long *)b);
        if (sbrk(-64 * 4096) == (void *)-1)
            fail("brk-shrink", 0, errno);
        /* regrown pages must read as ZERO (Linux semantics: fresh anon) */
        if (sbrk(16 * 4096) != (void *)-1) {
            char *z = (char *)sbrk(0) - 4096;
            for (int i = 0; i < 4096; i += 64)
                if (z[i] != 0) {
                    fail("brk-regrow-notzero", (unsigned long)&z[i], (unsigned char)z[i]);
                    break;
                }
            if (sbrk(-16 * 4096) == (void *)-1)
                fail("brk-shrink2", 0, errno);
        }
        __sync_fetch_and_add(&brk_rounds, 1);
    }
    return NULL;
}

int main(int argc, char **argv)
{
    int seconds = argc > 1 ? atoi(argv[1]) : 30;
    int nstack = argc > 2 ? atoi(argv[2]) : 3;

    setvbuf(stdout, NULL, _IONBF, 0);

    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_sigaction = sigsegv_handler;
    sa.sa_flags = SA_SIGINFO;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGSEGV, &sa, NULL) != 0 || sigaction(SIGBUS, &sa, NULL) != 0) {
        printf("VMAFAIL-sigaction errno=%d\n", errno);
        return 1;
    }

    printf("VMASTORM-START secs=%d stack_threads=%d\n", seconds, nstack);

    pthread_t dl, fk, bk, st[8];
    pthread_create(&dl, NULL, dlopen_thread, (void *)0L);
    pthread_create(&fk, NULL, fork_thread, (void *)0L);
    pthread_create(&bk, NULL, brk_thread, (void *)0L);
    if (nstack > 8)
        nstack = 8;
    for (int i = 0; i < nstack; i++)
        pthread_create(&st[i], NULL, stack_thread, (void *)(long)i);

    for (int s = 0; s < seconds; s++)
        sleep(1);
    stop_flag = 1;

    pthread_join(dl, NULL);
    pthread_join(fk, NULL);
    pthread_join(bk, NULL);
    for (int i = 0; i < nstack; i++)
        pthread_join(st[i], NULL);

    printf("VMASTORM-STATS dlopen=%lu stack=%lu fork=%lu brk=%lu fails=%lu\n",
           dlopen_rounds, stack_rounds, fork_rounds, brk_rounds, fail_count);
    if (fail_count) {
        printf("VMASTORM-FAIL\n");
        return 1;
    }
    printf("VMASTORM-PASS\n");
    return 0;
}
