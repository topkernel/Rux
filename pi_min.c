/* pi_min v3 — minimal RT priority-inversion probe for RT-2X item 7.
 *
 * Raw futex(2) LOCK_PI/UNLOCK_PI (no glibc PI mutex machinery), the
 * pi_stress group shape with correct RT discipline (no FIFO thread
 * ever spin-waits when idle — they park on a plain futex; only the
 * medium aggressor spins, which is its job):
 *
 *   high (FIFO 90) blocks on LOCK_PI held by low, signals completion
 *   med  (FIFO 80) spins in user mode the whole run (the aggressor)
 *   low  (FIFO 70) takes the PI lock, holds it 2 ms, releases
 *   main (FIFO 85) drives rounds, enforces a 3 s wedge budget with
 *                  nanosleep polls (blocks, never spins)
 *
 * Without RT priority inheritance: med(80) + high(90-blocked) occupy
 * both CPUs; low(70) never runs; the round never completes and the
 * budget fires. With the RT-2X boost low inherits 90 while high
 * waits, preempts med, releases; high's LOCK_PI wait stays sub-ms.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <sched.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>
#include <pthread.h>
#include <stdlib.h>

#define FUTEX_WAIT      0
#define FUTEX_WAKE      1
#define FUTEX_LOCK_PI   6
#define FUTEX_UNLOCK_PI 7
#define FUTEX_PRIVATE_FLAG 128

static volatile uint32_t lockw;   /* the PI mutex word */
static volatile uint32_t gow;     /* round sequencer: low/high park on change */
static volatile uint32_t donew;   /* high's completion sequencer */
static volatile uint32_t stopw;
static volatile uint64_t high_worst_us;
static volatile unsigned long rounds_done;
static int fails;

static long ftx(volatile uint32_t *a, int op, uint32_t val)
{
    return syscall(SYS_futex, (uint32_t *)a, op | FUTEX_PRIVATE_FLAG, val, 0, 0, 0);
}

static void wr(const char *s) { write(1, s, strlen(s)); }
static void wrn(const char *s, long v)
{
    char b[64];
    int n = snprintf(b, sizeof(b), "%s%ld\n", s, v);
    write(1, b, n);
}

static uint64_t now_us(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000 + ts.tv_nsec / 1000;
}

static void set_fifo(int prio)
{
    struct sched_param sp;
    memset(&sp, 0, sizeof(sp));
    sp.sched_priority = prio;
    if (syscall(SYS_sched_setscheduler, 0, SCHED_FIFO, &sp) != 0) {
        wrn("PIMIN: sched_setscheduler FIFO failed errno=", errno);
        _exit(8);
    }
}

static void spin_us(uint64_t us)
{
    uint64_t t0 = now_us();
    volatile uint64_t sink = 0;
    while (now_us() - t0 < us) sink++;
    (void)sink;
}

/* mode: 0 = PI lock (LOCK_PI/UNLOCK_PI), 1 = plain futex lock
 * (WAIT/WAKE under a held bit), 2 = no lock at all (pure FIFO
 * wake/park round trips). Selected by argv[1]. */
static int mode;
static volatile uint32_t plainw;

/* Sleep while *w == seq; return when the word CHANGES (the canonical
 * futex sequencer — v3 had this inverted, so the RT threads busy-spun
 * and starved each other: not a kernel bug). */
static void pin_cpu(int cpu)
{
    cpu_set_t set;
    CPU_ZERO(&set);
    CPU_SET(cpu, &set);
    if (sched_setaffinity(0, sizeof(set), &set) != 0)
        wrn("PIMIN: pin_cpu err=", errno);
}

static void park_until_change(volatile uint32_t *w, uint32_t *seqp)
{
    for (;;) {
        uint32_t cur = *w;
        if (cur != *seqp || stopw) { *seqp = cur; return; }
        ftx(w, FUTEX_WAIT, cur);
    }
}

static void *low_fn(void *arg)
{
    (void)arg;
    pin_cpu(0);
    set_fifo(70);
    uint32_t myseq = 0;
    for (;;) {
        park_until_change(&gow, &myseq);
        if (stopw) break;
        if (mode == 0) {
            if (ftx(&lockw, FUTEX_LOCK_PI, 0) != 0) { wr("PIMIN: low LOCK_PI err\n"); break; }
            spin_us(2000);  /* hold through the inversion window */
            if (ftx(&lockw, FUTEX_UNLOCK_PI, 0) != 0) { wr("PIMIN: low UNLOCK_PI err\n"); break; }
        } else if (mode == 1) {
            while (__sync_lock_test_and_set(&plainw, 1))
                ftx(&plainw, FUTEX_WAIT, 1);
            spin_us(2000);
            __sync_lock_release(&plainw);
            ftx(&plainw, FUTEX_WAKE, 1);
        }
    }
    return 0;
}

static void *med_fn(void *arg)
{
    (void)arg;
    pin_cpu(0);
    set_fifo(80);
    while (!stopw)
        spin_us(1000);   /* the aggressor: busy at 80 the whole run */
    return 0;
}

static void *high_fn(void *arg)
{
    (void)arg;
    pin_cpu(0);
    set_fifo(90);
    uint32_t myseq = 0;
    for (;;) {
        park_until_change(&gow, &myseq);
        if (stopw) break;
        if (mode != 2) {
            uint64_t w0 = now_us();
            if (mode == 0) {
                if (ftx(&lockw, FUTEX_LOCK_PI, 0) != 0) { wr("PIMIN: high LOCK_PI err\n"); break; }
            } else {
                while (__sync_lock_test_and_set(&plainw, 1))
                    ftx(&plainw, FUTEX_WAIT, 1);
            }
            uint64_t dt = now_us() - w0;
            if (dt > high_worst_us) high_worst_us = dt;
            if (mode == 0) {
                if (ftx(&lockw, FUTEX_UNLOCK_PI, 0) != 0) { wr("PIMIN: high UNLOCK_PI err\n"); break; }
            } else {
                __sync_lock_release(&plainw);
                ftx(&plainw, FUTEX_WAKE, 1);
            }
        }
        donew = myseq;
        ftx(&donew, FUTEX_WAKE, 1);
    }
    return 0;
}

/* mode/plainw decls moved above thread fns */

int main(int argc, char **argv)
{
    long r;
    mode = (argc > 1) ? atoi(argv[1]) : 0;
    wr(mode == 0 ? "PIMIN: mode=PI LOCK_PI\n"
       : mode == 1 ? "PIMIN: mode=plain futex lock\n"
       : "PIMIN: mode=no lock (wake/park only)\n");
    pin_cpu(1);     /* main on the other cpu: budget enforcement */
    set_fifo(85);   /* above the aggressor so budgets always enforce */
    if (mode == 2) goto run_rounds;
    if (mode == 1) {
        /* plain-lock sanity: bit 0 = held */
        plainw = 0;
    }

    lockw = 0; gow = 0; donew = 0; stopw = 0;
    if (mode == 0) {
        r = ftx(&lockw, FUTEX_LOCK_PI, 0);
        wr(r == 0 ? "PIMIN: uncontended LOCK_PI ok\n" : "PIMIN: uncontended LOCK_PI FAIL\n");
        if (r != 0) fails++;
        r = ftx(&lockw, FUTEX_UNLOCK_PI, 0);
        wr(r == 0 ? "PIMIN: uncontended UNLOCK_PI ok\n" : "PIMIN: uncontended UNLOCK_PI FAIL\n");
        if (r != 0) fails++;
    }

run_rounds:;
    pthread_t tl, tm, th;
    if (pthread_create(&tl, 0, low_fn, 0) != 0) { wr("PIMIN: pthread low FAIL\n"); return 9; }
    if (pthread_create(&tm, 0, med_fn, 0) != 0) { wr("PIMIN: pthread med FAIL\n"); return 9; }
    if (pthread_create(&th, 0, high_fn, 0) != 0) { wr("PIMIN: pthread high FAIL\n"); return 9; }
    wr("PIMIN: threads up\n");

    struct timespec ns50 = { 0, 50 * 1000 };
    uint32_t seq = 0;
    for (int i = 0; i < 200 && !stopw; i++) {
        seq += 1;
        donew = 0;
        gow = seq;
        ftx(&gow, FUTEX_WAKE, 2);       /* release low + high */
        uint64_t w0 = now_us();
        while (donew != seq && !stopw) {
            if (now_us() - w0 > 3000000ULL) {
                wrn("PIMIN: ROUND STUCK >3s at round ", i);
                wr("PIMIN: PI boost failed to break the inversion\n");
                stopw = 1;
                fails++;
                break;
            }
            nanosleep(&ns50, 0);        /* block, never spin */
        }
        rounds_done++;
    }
    stopw = 1;
    gow = 0xffffffff;
    ftx(&gow, FUTEX_WAKE, 2);
    nanosleep(&(struct timespec){ 0, 100 * 1000 * 1000 / 2 }, 0);

    if (fails == 0 && rounds_done >= 100) {
        wrn("PIMIN: PASS mode=", mode);
    wrn("PIMIN: PASS rounds=", (long)rounds_done);
        wrn("PIMIN: worst high LOCK_PI wait us=", (long)high_worst_us);
        return 0;
    }
    wrn("PIMIN: FAIL mode=", mode);
    wrn("PIMIN: rounds completed=", (long)rounds_done);
    return 1;
}
