// gwp_churn.c — GWP-Asan-style mprotect/fault churn (the OH
// "pagefault: Permission denied at rip=ld.so+0x89ac6" family).
//
// OH's allocator reserves ~2 MB PROT_NONE anonymous regions, then
// mprotects 4 KB slots to PROT_READ (metadata table reads, the observed
// fault PCs read at region_start+0 / region_start+4) or PROT_READ|WRITE
// (sampled allocations) and back to PROT_NONE. Services died with
// SIGSEGV/EACCES on pages whose VMA showed flags=0x10 (PRIVATE only —
// every prot bit stripped) or with NO covering VMA at all, meaning an
// mprotect upgrade was lost or a VMA piece vanished.
//
// This probe hammers every transition with concurrent faults and fork
// COW, and treats ANY of the following as a failure:
//   G1. mprotect returning an unexpected error (or unexpected success
//       semantics: an upgrade that returns 0 but leaves the slot
//       non-accessible is caught by the touch below).
//   G2. a fault (SIGSEGV) on a slot whose current published state says
//       it MUST be accessible.
//   G3. canary corruption inside an RW slot.
//
// Topology:
//   - REGION: 2 MB PROT_NONE anonymous (mmap MAP_PRIVATE|MAP_ANON).
//   - 4 worker "clones" (share mm): random slot transitions
//       NONE -> RW -> write canary -> verify -> R -> read verify -> NONE,
//     plus periodic first-page PROT_READ table reads (+0/+4 words).
//   - 1 forker: fork children that snapshot the published-slot table and
//     read every slot published readable; COW + PROT_NONE mixture.
//   - main: progress + verdict.
//
// PASS: GWP-PASS after <secs>; any GWPFAIL-* line = rc 1 (driver).
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sched.h>
#include <pthread.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define REGION_SZ   (2UL << 20)
#define PAGE        4096UL
#define NSLOTS      (REGION_SZ / PAGE)     /* 512 */
#define NWORKERS    4

/* published slot state — written by the owning worker BEFORE unmprotect
 * and AFTER re-protecting; readers/children only touch slots whose
 * published state is readable. 0=NONE 1=R 2=RW */
static volatile uint8_t slot_state[NSLOTS];
static volatile uint8_t slot_salt[NSLOTS];
static volatile int stop_flag;
static unsigned long ops_total, forks_done;
static char *region;

static void fail(const char *what, unsigned long a, unsigned long b)
{
    char buf[160];
    int n = snprintf(buf, sizeof(buf), "GWPFAIL-%s a=%#lx b=%#lx errno=%d\n",
                     what, a, b, errno);
    if (n > 0)
        (void)write(2, buf, n);
    _exit(42);
}

static inline uint64_t canary(unsigned long slot, uint8_t salt)
{
    return 0x9e3779b97f4a7c15ULL ^ (uint64_t)(region + slot * PAGE)
           ^ ((uint64_t)salt << 32) ^ (slot * 11400714819323198485ULL);
}

static void sigsegv_handler(int sig, siginfo_t *si, void *ctx)
{
    (void)sig; (void)ctx;
    char buf[160];
    unsigned long off = (char *)si->si_addr - region;
    unsigned long slot = off < REGION_SZ ? off / PAGE : ~0UL;
    int st = slot < NSLOTS ? slot_state[slot] : -1;
    int n = snprintf(buf, sizeof(buf),
                     "GWPFAIL-SIGSEGV addr=%p slot=%lu published_state=%d\n",
                     si->si_addr, slot, st);
    if (n > 0)
        (void)write(2, buf, n);
    _exit(42);
}

static void mprot(void *addr, size_t len, int prot, const char *tag)
{
    if (mprotect(addr, len, prot) != 0)
        fail(tag, (unsigned long)addr, prot);
}

static volatile int pause_churn;       /* forker requests quiescence */
static volatile int active_workers;     /* workers inside a transition */

static void worker(void *wid_ptr)
{
    int wid = *(volatile int *)wid_ptr;
    unsigned seed = (unsigned)(wid * 7919 + 1) | 1;
    /* DISJOINT slot partition per worker: rand collisions between
     * workers made one worker's R->NONE cycle race another's RW->store
     * on the SAME slot (a userspace false positive, not a kernel bug —
     * observed run 2: pid 306 stalled mid-cycle while pid 304 completed
     * a full RW->R->NONE cycle on the same address). */
    unsigned slots_per = NSLOTS / NWORKERS;
    while (!stop_flag) {
        /* quiescence point: honor the forker's pause before the next
         * slot transition, so a fork child can never race a slot that
         * is about to lose its prot (false-positive free). Increment
         * FIRST, then re-check pause: the forker's active==0 read then
         * covers the check-pause/decide window. */
        __sync_fetch_and_add(&active_workers, 1);
        __sync_synchronize();
        if (pause_churn) {
            __sync_fetch_and_sub(&active_workers, 1);
            while (pause_churn && !stop_flag) {
                struct timespec ts = {0, 2000000}; /* 2 ms */
                nanosleep(&ts, NULL);
            }
            continue;
        }
        if (stop_flag) {
            __sync_fetch_and_sub(&active_workers, 1);
            break;
        }

        unsigned long slot = (rand_r(&seed) % slots_per) * NWORKERS + (unsigned long)wid;
        char *p = region + slot * PAGE;

        /* NONE -> RW: publish AFTER mprotect success */
        mprot(p, PAGE, PROT_READ | PROT_WRITE, "mp-rw");
        __sync_synchronize();
        slot_salt[slot] = (uint8_t)slot;
        slot_state[slot] = 2;
        uint64_t c = canary(slot, slot_salt[slot]);
        *(volatile uint64_t *)p = c;
        if (*(volatile uint64_t *)p != c)
            fail("canary-rw", slot, 0);

        /* RW -> R: still readable, not writable */
        mprot(p, PAGE, PROT_READ, "mp-r");
        slot_state[slot] = 1;
        __sync_synchronize();
        (void)*(volatile uint64_t *)p;

        /* R -> NONE */
        mprot(p, PAGE, PROT_NONE, "mp-none");
        slot_state[slot] = 0;
        __sync_synchronize();

        /* table-read pattern: first page of the region readable briefly */
        if ((slot & 31) == 0) {
            mprot(region, PAGE, PROT_READ, "mp-table");
            __sync_synchronize();
            (void)*(volatile uint32_t *)region;
            (void)*((volatile uint32_t *)region + 1); /* +4 — the OH PC */
            mprot(region, PAGE, PROT_NONE, "mp-table-none");
            __sync_synchronize();
        }
        __sync_fetch_and_add(&ops_total, 1);
        __sync_synchronize();
        __sync_fetch_and_sub(&active_workers, 1);
        __sync_synchronize();
    }
}

static void forker(void *unused)
{
    (void)unused;
    while (!stop_flag) {
        /* quiesce the workers, then fork with the slot table stable */
        pause_churn = 1;
        while (active_workers > 0 && !stop_flag) {
            struct timespec ts = {0, 2000000 };
            nanosleep(&ts, NULL);
        }
        if (stop_flag)
            break;
        pid_t pid = fork();
        if (pid == 0) {
            /* child: read every slot published readable at snapshot */
            for (unsigned long s = 0; s < NSLOTS; s++) {
                if (slot_state[s] >= 1) {
                    uint64_t v = *(volatile uint64_t *)(region + s * PAGE);
                    if (v != canary(s, slot_salt[s]))
                        fail("canary-child", s, (unsigned long)v);
                }
            }
            _exit(0);
        }
        if (pid < 0)
            fail("fork", 0, 0);
        pause_churn = 0;
        int st;
        waitpid(pid, &st, 0);
        if (!(WIFEXITED(st) && WEXITSTATUS(st) == 0)) {
            char buf[96];
            int n = snprintf(buf, sizeof(buf), "GWPFAIL-child st=%d\n", st);
            if (n > 0)
                (void)write(2, buf, n);
            _exit(42);
        }
        __sync_fetch_and_add(&forks_done, 1);
    }
}

/* pthreads: raw clone() with glibc's syscall() wrapper returns through
 * the parent stack frame in the child (garbage RSP) — use real threads. */

int main(int argc, char **argv)
{
    int secs = 20;
    if (argc > 1)
        secs = atoi(argv[1]);
    setvbuf(stdout, NULL, _IONBF, 0);

    struct sigaction sa = {0};
    sa.sa_sigaction = sigsegv_handler;
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGSEGV, &sa, NULL);
    sigaction(SIGBUS, &sa, NULL);

    region = mmap(NULL, REGION_SZ, PROT_NONE,
                  MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (region == MAP_FAILED)
        fail("mmap-region", 0, 0);
    printf("gwp_churn: region=%p secs=%d\n", region, secs);

    static int wids[NWORKERS];
    pthread_t th[NWORKERS + 1];
    for (int i = 0; i < NWORKERS; i++) {
        wids[i] = i;
        if (pthread_create(&th[i], NULL, (void *(*)(void *))worker, &wids[i]) != 0)
            fail("pthread-worker", i, 0);
    }
    if (pthread_create(&th[NWORKERS], NULL, (void *(*)(void *))forker, NULL) != 0)
        fail("pthread-forker", 0, 0);

    for (int s = 0; s < secs && !stop_flag; s += 5) {
        struct timespec ts = {5, 0};
        nanosleep(&ts, NULL);
        printf("gwp_churn: t+%ds ops=%lu forks=%lu\n",
               s + 5, ops_total, forks_done);
    }
    stop_flag = 1;
    struct timespec ts = {1, 0};
    nanosleep(&ts, NULL);
    printf("GWP-PASS ops=%lu forks=%lu\n", ops_total, forks_done);
    return 0;
}
