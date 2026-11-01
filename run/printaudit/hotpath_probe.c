/* hotpath_probe.c — print-audit lane A/B probe (x86_64 musl static).
 *
 * PID-1 boot (init=/test/hotpath_probe): measures the per-op cost of
 * the syscall families the print audit re-gated:
 *   1. futex WAKE uncontended   (do_futex prologue: FXDV name-compare)
 *   2. tgkill to self + handler (send_signal_from_user_tkill: TKSIG)
 *   3. mprotect RW<->NONE flip  (sys_mprotect: MPROTECT-RW)
 *   4. mmap+munmap 1 MiB anon   (sys_munmap: BIGMUNMAP)
 * Prints one "HPROBE <case> <ns/op>" line per case; medians over
 * batches so a single hiccup cannot skew a case.
 */
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <signal.h>
#include <string.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/syscall.h>

#define BATCH 100
#define NBATCH 40

static long long ts[NBATCH];

static long long now_ns(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (long long)t.tv_sec * 1000000000LL + t.tv_nsec;
}

static int cmp(const void *a, const void *b)
{
    long long x = *(const long long *)a, y = *(const long long *)b;
    return x < y ? -1 : x > y ? 1 : 0;
}

static void report(const char *name)
{
    qsort(ts, NBATCH, sizeof(long long), cmp);
    printf("HPROBE %s %.1f\n", name, (double)ts[NBATCH / 2] / BATCH);
    fflush(stdout);
}

static void handler(int s) { (void)s; }

int main(void)
{
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = handler;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGUSR1, &sa, NULL);

    /* 1. futex WAKE, uncontended */
    {
        unsigned int word = 0;
        for (int b = 0; b < NBATCH; b++) {
            long long t0 = now_ns();
            for (int i = 0; i < BATCH; i++)
                syscall(SYS_futex, &word, 1 /*FUTEX_WAKE*/, 1, 0, 0, 0);
            ts[b] = now_ns() - t0;
        }
        report("futex_wake");
    }

    /* 2. tgkill to self with a handler installed */
    {
        pid_t pid = getpid();
        pid_t tid = syscall(SYS_gettid);
        for (int b = 0; b < NBATCH; b++) {
            long long t0 = now_ns();
            for (int i = 0; i < BATCH; i++)
                syscall(SYS_tgkill, pid, tid, SIGUSR1);
            ts[b] = now_ns() - t0;
        }
        report("tgkill_catch");
    }

    /* 3. mprotect RW <-> NONE flip over 1 MiB */
    {
        void *p = mmap(NULL, 1 << 20, PROT_NONE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (p == MAP_FAILED) {
            printf("HPROBE mprotect_flip MAP_FAILED\n");
            return 1;
        }
        for (int b = 0; b < NBATCH; b++) {
            long long t0 = now_ns();
            for (int i = 0; i < BATCH; i++) {
                mprotect(p, 1 << 20, PROT_READ | PROT_WRITE);
                mprotect(p, 1 << 20, PROT_NONE);
            }
            ts[b] = now_ns() - t0;
        }
        report("mprotect_flip");
    }

    /* 4. mmap + munmap of 1 MiB anon (thread-stack shape) */
    {
        for (int b = 0; b < NBATCH; b++) {
            long long t0 = now_ns();
            for (int i = 0; i < BATCH; i++) {
                void *p = mmap(NULL, 1 << 20, PROT_READ | PROT_WRITE,
                               MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
                munmap(p, 1 << 20);
            }
            ts[b] = now_ns() - t0;
        }
        report("mmap_munmap_1m");
    }

    printf("HPROBE done\n");
    return 0;
}
