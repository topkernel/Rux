/*
 * futex_min2.c — definitive FUTEX_WAIT blocking probe (no printing from
 * the worker; everything recorded and printed by the parent after join).
 *
 * Records per WAIT: return value, errno, userspace word value at return,
 * and a monotonically increasing wait counter (parent snapshots it
 * before/after WAKE).
 */
#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <linux/futex.h>

static long fx(volatile uint32_t *a, int op, uint32_t v)
{
    return syscall(SYS_futex, (uint32_t *)a, op, v, NULL, NULL, 0);
}

static volatile uint32_t word;
static volatile int nwait_calls;
static volatile int woke_by_wake;
struct rec {
    long ret;
    int err;
    uint32_t w;
    int ncall;
};
static struct rec recs[64];
static volatile int nrec;

static void *worker(void *arg)
{
    (void)arg;
    while (word == 0) {
        int mycall = __sync_fetch_and_add(&nwait_calls, 1) + 1;
        errno = 0;
        long r = fx(&word, FUTEX_WAIT, 0);
        int e = errno;
        if (r == 0)
            __sync_fetch_and_add(&woke_by_wake, 1);
        int i = __sync_fetch_and_add(&nrec, 1);
        if (i < 64) {
            recs[i].ret = r; recs[i].err = e; recs[i].w = word; recs[i].ncall = mycall;
        }
    }
    return NULL;
}

int main(int argc, char **argv)
{
    int n = argc > 1 ? atoi(argv[1]) : 2;
    pthread_t th[32];
    word = 0; nwait_calls = 0; woke_by_wake = 0; nrec = 0;
    for (int i = 0; i < n; i++)
        pthread_create(&th[i], NULL, worker, NULL);
    usleep(300000);
    int before = nwait_calls;
    word = 1;
    long wr = fx(&word, FUTEX_WAKE, n);
    int after = nwait_calls;
    for (int i = 0; i < n; i++)
        pthread_join(th[i], NULL);
    printf("wait-calls before wake: %d, after wake: %d (total %d)\n", before, after, nwait_calls);
    printf("WAKE syscall returned %ld; waits that returned 0 (properly woken): %d\n", wr, woke_by_wake);
    for (int i = 0; i < nrec && i < 16; i++)
        printf("rec[%d]: call#%d ret=%ld errno=%d word=%u\n", i, recs[i].ncall, recs[i].ret, recs[i].err, recs[i].w);
    return 0;
}
