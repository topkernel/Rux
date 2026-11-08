/* jitstress — deterministic reproducer for the llvmpipe JIT corruption.
 *
 * Failure chain under test (gnome-shell boots): llvmpipe spawns 8 render
 * threads; each mmaps RW regions, writes code+data, mprotects to R-X,
 * executes — while sibling threads churn mmap/munmap.  ~30-50% of boots
 * die with "LLVM ERROR: 64-bit code requested on a subtarget that
 * doesn't support it!" (the JIT's in-memory data corrupted at runtime)
 * and the shell abort()s.
 *
 * Suspect kernel window (kernel/src/arch/x86_64/mm/mm_ops.rs, munmap):
 * ranges > 4MB zap their PTEs AFTER the vma_write guard drops
 * ("deferred zap").  A racing same-mm mmap that lands in the finished
 * hole installs a fresh VMA; the owner writes through it; the deferred
 * zap then clears the FRESH PTEs, put_page()s the fresh frames (they
 * get reused -> cross-mapping aliasing) and the owner's refault
 * zero-fills — writes LOST, memory CLOBBERED.  The code comment claims
 * the fault path "self-heals"; for anonymous memory that was already
 * written it does not — the data is gone and the frame was freed while
 * still mapped.
 *
 * Probe shape (8 threads):
 *   thread 0  "churner":  mmap MAP_FIXED BASE(16MB, RW) -> dirty all pages
 *                         -> phase=RACING -> munmap() (16MB > 4MB = the
 *                         deferred-zap path) -> phase=VERIFY -> wait for
 *                         landers -> next round.
 *   threads 1-4 "landers": while RACING, install MAP_FIXED mappings into
 *                         the arena (own disjoint 512KB slots), write a
 *                         tag pattern, verify IMMEDIATELY (LOSTWRITE
 *                         face); keep them live; at VERIFY re-check every
 *                         tag (ZAPCLEAR face — the churner's munmap has
 *                         RETURNED, nothing legal can change them now).
 *   threads 5-7 "jit":    llvmpipe shape far from the arena: mmap RW,
 *                         write stub + data blob + canary, mprotect the
 *                         first 2 pages R|X, EXECUTE, verify data, drop
 *                         back to RW, verify canary, munmap.  Traps on
 *                         execute are caught (sigsetjmp) -> EXECFAIL.
 *
 * Detector classes:
 *   LOSTWRITE  read-back right after the write returned wrong bytes
 *   ZAPCLEAR   a live mapping's bytes changed while nothing legal
 *              touched it (munmap already returned; churner waits)
 *   EXECFAIL   executing the freshly written stub trapped or misran
 *   VANISH     mapping legally removed by an overlapping munmap race
 *              (SIGSEGV during verify; counted, not a failure)
 *
 * Usage: jitstress [rounds]
 * Exit code = number of failure classes seen (capped at 125).
 */
#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <setjmp.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define BIGSZ    (16u << 20)          /* churn region: > 4MB single-guard limit */
#define BASE     0x400000000000ULL    /* dedicated arena, far from libc's mmap area */
#define NSLOT    32                   /* lander slots inside the arena */
#define SLOTSZ   (BIGSZ / NSLOT)      /* 512KB each */
#define ROUNDS   4000
#define JIT_ROUNDS 200000

enum { PH_IDLE = 0, PH_RACING = 1, PH_VERIFY = 2 };

static volatile int phase = PH_IDLE;
static volatile int rounds_done;
static volatile int stop_all;
static volatile int landers_busy;      /* landers still inside a round */
static volatile long n_lostwrite, n_zapclear, n_execfail, n_vanish;

#define SLOT_OWNER(s) (((s) % 4) + 1)

/* thread-local trap protection: verify/fill of arena ranges can legally
 * SIGSEGV when an overlapping munmap removed the VMA mid-round; catch,
 * mark, continue.  Outside a protected region a trap is a real bug —
 * restore SIG_DFL and re-raise so the serial log shows it. */
static __thread int in_protect;
static __thread sigjmp_buf protect_jmp;
static volatile uintptr_t trap_rip, trap_addr;

static void trap_handler(int sig, siginfo_t *si, void *uc)
{
    ucontext_t *u = uc;
    trap_rip = u->uc_mcontext.gregs[REG_RIP];
    trap_addr = (uintptr_t)si->si_addr;
    if (in_protect) {
        in_protect = 0;
        siglongjmp(protect_jmp, sig);
    }
    struct sigaction dfl = { 0 };
    dfl.sa_handler = SIG_DFL;
    sigemptyset(&dfl.sa_mask);
    sigaction(sig, &dfl, NULL);
    raise(sig);
}

/* Runs `call` trap-protected; sets `caught` to the signal number if a
 * trap fired inside, 0 otherwise. */
#define PROTECT(call, caught)                                 \
    do {                                                      \
        (caught) = sigsetjmp(protect_jmp, 1);                 \
        if ((caught) == 0) {                                  \
            in_protect = 1;                                   \
            call;                                             \
            in_protect = 0;                                   \
        }                                                     \
    } while (0)

/* ---------- churner: open/close the giant deferred-zap window ---------- */
static void *churner(void *arg)
{
    (void)arg;
    for (int r = 0; r < ROUNDS && !stop_all; r++) {
        /* wait until every lander finished its previous round */
        int spin = 0;
        while (__atomic_load_n(&landers_busy, __ATOMIC_ACQUIRE) != 0
               && spin++ < 400000000)
            __asm__ volatile("pause");

        void *m = mmap((void *)BASE, BIGSZ, PROT_READ | PROT_WRITE,
                       MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (m == MAP_FAILED) {
            printf("JITSTRESS churner mmap FAILED %s\n", strerror(errno));
            stop_all = 1;
            return NULL;
        }
        /* dirty every page so the munmap zap has real PTEs to walk */
        for (unsigned off = 0; off < BIGSZ; off += 4096)
            *(volatile uint8_t *)((char *)BASE + off) = (uint8_t)(off >> 12);

        /* the W^X dance llvmpipe does on its slabs */
        mprotect((void *)BASE, 4096, PROT_READ);
        mprotect((void *)BASE, 4096, PROT_READ | PROT_WRITE);

        __atomic_store_n(&phase, PH_RACING, __ATOMIC_RELEASE);
        munmap((void *)BASE, BIGSZ);   /* > 4MB: deferred-zap path */
        __atomic_store_n(&phase, PH_VERIFY, __ATOMIC_RELEASE);

        spin = 0;
        while (__atomic_load_n(&landers_busy, __ATOMIC_ACQUIRE) != 0
               && spin++ < 400000000)
            __asm__ volatile("pause");
        __atomic_store_n(&phase, PH_IDLE, __ATOMIC_RELEASE);

        rounds_done = r + 1;
        if (r % 500 == 0) {
            printf("JITSTRESS round %d lost=%ld zap=%ld exec=%ld van=%ld\n",
                   r, n_lostwrite, n_zapclear, n_execfail, n_vanish);
        }
    }
    stop_all = 1;
    return NULL;
}

/* ---------- lander: race fresh mappings into the pending hole ---------- */
static void fill_range(char *p, size_t len, uint64_t tag)
{
    for (size_t off = 0; off < len; off += 512)
        memcpy(p + off, &tag, 8);
}

/* returns -1 ok, else index of the first bad 512-byte block */
static long verify_range(char *p, size_t len, uint64_t tag)
{
    for (size_t off = 0; off < len; off += 512) {
        uint64_t v;
        memcpy(&v, p + off, 8);
        if (v != tag) return (long)off;
    }
    return -1;
}

#define MAX_LIVE 8
struct live_rec { char *addr; size_t len; uint64_t tag; int slot; };

static void *lander(void *arg)
{
    long id = (long)arg;
    unsigned seed = (unsigned)id * 7919u + 17;
    struct live_rec live[MAX_LIVE];
    int n_live;

    while (!stop_all) {
        while (__atomic_load_n(&phase, __ATOMIC_ACQUIRE) != PH_RACING
               && !stop_all)
            __asm__ volatile("pause");
        if (stop_all) break;
        __atomic_fetch_add(&landers_busy, 1, __ATOMIC_ACQ_REL);
        n_live = 0;

        for (int k = 0; k < 6; k++) {
            seed = seed * 1103515245 + 12345;
            int s = -1;
            for (int t = 0; t < NSLOT; t++) {
                int cand = (seed + t) % NSLOT;
                if (SLOT_OWNER(cand) == id) { s = cand; break; }
            }
            if (s < 0) break;
            char *a = (char *)(BASE + (uint64_t)s * SLOTSZ);
            size_t len = SLOTSZ; /* full slot; slots never overlap */
            int caught = 0;

            PROTECT(
                do {
                    void *m = mmap(a, len, PROT_READ | PROT_WRITE,
                                   MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
                    if (m == MAP_FAILED) break;
                    uint64_t tag = ((uint64_t)(rounds_done + 1) << 32)
                                 | ((uint64_t)s << 8) | (uint64_t)id;
                    fill_range(a, len, tag);
                    __sync_synchronize();
                    long bad = verify_range(a, len, tag);
                    if (bad >= 0) {
                        n_lostwrite++;
                        if (n_lostwrite <= 5)
                            printf("JITSTRESS LOSTWRITE slot=%d round=%d off=%ld tag=%llx got=%llx\n",
                                   s, rounds_done, bad,
                                   (unsigned long long)tag,
                                   *(unsigned long long *)(a + bad));
                    } else if (n_live < MAX_LIVE) {
                        live[n_live].addr = a; live[n_live].len = len;
                        live[n_live].tag = tag; live[n_live].slot = s;
                        n_live++;
                    } else {
                        munmap(a, len);
                    }
                } while (0);
            , caught);
            if (caught) {
                n_vanish++;
                if (n_vanish <= 5)
                    printf("JITSTRESS VANISH(fill) slot=%d sig=%d\n", s, caught);
            }
            if (__atomic_load_n(&phase, __ATOMIC_ACQUIRE) != PH_RACING
                && !stop_all)
                break;
        }

        /* churner's munmap has returned once we see VERIFY: re-check every
         * live mapping — nothing legal can mutate them now */
        while (__atomic_load_n(&phase, __ATOMIC_ACQUIRE) == PH_RACING
               && !stop_all)
            __asm__ volatile("pause");
        for (int i = 0; i < n_live; i++) {
            int caught = 0;
            PROTECT(
                do {
                    long bad = verify_range(live[i].addr, live[i].len, live[i].tag);
                    if (bad >= 0) {
                        n_zapclear++;
                        if (n_zapclear <= 8)
                            printf("JITSTRESS ZAPCLEAR slot=%d round=%d off=%ld tag=%llx got=%llx\n",
                                   live[i].slot, rounds_done, bad,
                                   (unsigned long long)live[i].tag,
                                   *(unsigned long long *)(live[i].addr + bad));
                    }
                    munmap(live[i].addr, live[i].len);
                } while (0);
            , caught);
            if (caught)
                n_vanish++;
        }
        __atomic_fetch_sub(&landers_busy, 1, __ATOMIC_ACQ_REL);
    }
    return NULL;
}

/* ---------- jit: the llvmpipe compile->W^X->execute shape ---------- */
struct jit_ctx { long id; long exec_ok; };

static void *jit_thread(void *arg)
{
    struct jit_ctx *ctx = arg;
    unsigned seed = (unsigned)ctx->id * 104729u + 3;
    static const char blob[] = "x86_64/features:+64bit,+avx2,+sse4.2";
    for (int r = 0; r < JIT_ROUNDS && !stop_all; r++) {
        seed = seed * 1103515245 + 12345;
        size_t len = (1u + (seed % 4)) << 20;   /* 1-4MB RW slab */
        char *m = mmap(NULL, len, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (m == MAP_FAILED) continue;
        unsigned magic = 0xC0DE0000u + (seed & 0xffff);
        unsigned char code[] = {
            0xb8, (unsigned char)magic, (unsigned char)(magic >> 8),
            (unsigned char)(magic >> 16), (unsigned char)(magic >> 24), 0xc3,
        };
        memcpy(m, code, sizeof code);
        memcpy(m + 4096, blob, sizeof blob);
        uint64_t checksum = 0;
        for (size_t i = 0; i < sizeof blob; i++)
            checksum = checksum * 131 + (unsigned char)m[4096 + i];
        for (size_t off = 8192; off < len; off += 512)
            memcpy(m + off, &checksum, 8);

        if (mprotect(m, 2 * 4096, PROT_READ | PROT_EXEC) != 0) {
            munmap(m, len);
            continue;
        }
        int caught = 0;
        unsigned rv = 0;
        PROTECT(rv = ((unsigned (*)(void))m)();, caught);
        if (caught) {
            n_execfail++;
            if (n_execfail <= 8)
                printf("JITSTRESS EXECFAIL sig=%d rip=%lx addr=%lx round=%d\n",
                       caught, (unsigned long)trap_rip,
                       (unsigned long)trap_addr, r);
        } else if (rv != magic) {
            n_execfail++;
            if (n_execfail <= 5)
                printf("JITSTRESS EXECBAD rv=%08x want=%08x round=%d\n",
                       rv, magic, r);
        } else {
            ctx->exec_ok++;
        }
        int data_bad = 0;
        PROTECT(data_bad = memcmp(m + 4096, blob, sizeof blob - 1) != 0;, caught);
        if (caught) {
            n_vanish++;
        } else if (data_bad) {
            n_zapclear++;
            if (n_zapclear <= 8)
                printf("JITSTRESS DATABLOB round=%d got=%.36s\n", r, m + 4096);
        }
        mprotect(m, 2 * 4096, PROT_READ | PROT_WRITE);
        int can_bad = 0;
        PROTECT(
            do {
                for (size_t off = 8192; off < len; off += 512) {
                    uint64_t v;
                    memcpy(&v, m + off, 8);
                    if (v != checksum) { can_bad = 1; break; }
                }
            } while (0);
        , caught);
        if (caught) {
            n_vanish++;
        } else if (can_bad) {
            n_zapclear++;
            if (n_zapclear <= 8)
                printf("JITSTRESS JITCANARY round=%d\n", r);
        }
        munmap(m, len);
        if (r % 50000 == 0)
            printf("JITSTRESS jit%ld round=%d ok=%ld\n", ctx->id, r, ctx->exec_ok);
    }
    return NULL;
}

int main(int argc, char **argv)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    int rounds = ROUNDS;
    if (argc > 1) rounds = atoi(argv[1]);
    printf("JITSTRESS start pid=%d rounds=%d arena=%llx..%llx\n",
           getpid(), rounds, (unsigned long long)BASE,
           (unsigned long long)(BASE + BIGSZ));

    struct sigaction sa = { 0 };
    sa.sa_sigaction = trap_handler;
    sa.sa_flags = SA_SIGINFO;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGSEGV, &sa, NULL);
    sigaction(SIGILL, &sa, NULL);
    sigaction(SIGBUS, &sa, NULL);

    pthread_t th[8];
    struct jit_ctx jctx[3] = {{0, 0}, {1, 0}, {2, 0}};

    pthread_create(&th[0], NULL, churner, NULL);
    for (long i = 1; i <= 4; i++)
        pthread_create(&th[i], NULL, lander, (void *)i);
    for (long i = 0; i < 3; i++)
        pthread_create(&th[5 + i], NULL, jit_thread, &jctx[i]);

    for (int i = 0; i < 8; i++)
        pthread_join(th[i], NULL);

    long fails = (n_lostwrite > 0) + (n_zapclear > 0) + (n_execfail > 0);
    printf("JITSTRESS-DONE rounds=%d lostwrite=%ld zapclear=%ld execfail=%ld "
           "exec_ok=%ld/%ld/%ld vanish=%ld VERDICT=%s\n",
           rounds_done, n_lostwrite, n_zapclear, n_execfail,
           jctx[0].exec_ok, jctx[1].exec_ok, jctx[2].exec_ok, n_vanish,
           fails ? "CORRUPT" : "CLEAN");
    return (int)(fails > 125 ? 125 : fails);
}
