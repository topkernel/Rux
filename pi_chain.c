// pi_chain.c — N-deep PI chain propagation probe (R6, rt-r6 lane).
//
// Topology per round (DEPTH = 3 locks, DEPTH+1 worker threads):
//
//   T0 (SCHED_OTHER nice 0) holds L0
//   T1 (FIFO 40) holds L1, blocks acquiring L0
//   T2 (FIFO 60) holds L2, blocks acquiring L1
//   T3 (FIFO 90) blocks acquiring L2
//
// With numeric priority propagation along the chain (the reference's
// rt_mutex_adjust_prio_chain walk), while the chain is up:
//   - T0 is CONVERTED to SCHED_FIFO at 90 (the rt_mutex_setprio shape:
//     a CFS owner under an RT top waiter)
//   - T1, T2 run at FIFO 90 (raised from 40/60 — the boost walked
//     THROUGH them, which the pre-R6 single-edge engine cannot do)
// The monitor (main thread) verifies all three via
// sched_getscheduler/sched_getparam while the chain is assembled, then
// releases it; T3's acquire latency must stay bounded (no starve).
//
// Orchestration uses PLAIN futex WAIT/WAKE gates exclusively (no
// nanosleep polls): a pre-existing short-nanosleep wake-loss bug on
// main (see run/RT-R6.md, "pre-existing lost-wake") wedges any
// msleep-polled orchestration within seconds on BOTH the base and the
// R6 kernel — it is not a PI-chain artifact.
//
// Usage: pi_chain [rounds=200]
// Verdict: RES|pi_chain|PASS|rounds=<n> boost_ok=<n> starved=<n> ...

#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <pthread.h>
#include <sched.h>
#include <time.h>
#include <sys/syscall.h>
#include <linux/futex.h>
#include <stdint.h>

#define DEPTH 3

static volatile uint32_t lock_word[DEPTH]; // PI futex words (owner tid)
// Plain-futex orchestration gates.
static volatile uint32_t gate_round;       // 1 = round running, 0 = reset
static volatile uint32_t gate_tier[DEPTH + 1]; // tier i opens at 1 (T3 = gate 3)
static volatile uint32_t gate_release;     // T0 releases at 1
static volatile int done_count, back_count, stop_now;

static long futex_op(volatile uint32_t *uaddr, int op, uint32_t val)
{
    return syscall(SYS_futex, (uint32_t *)uaddr, op, val, NULL, NULL, 0);
}

/// Sleep until *addr != expect (plain futex wait; returns on wake or
/// value change).
static void gate_wait(volatile uint32_t *addr, uint32_t expect)
{
    for (;;) {
        uint32_t v = *addr;
        if (v != expect)
            return;
        futex_op(addr, FUTEX_WAIT, v);
    }
}

static void gate_open(volatile uint32_t *addr, uint32_t v)
{
    *addr = v;
    futex_op(addr, FUTEX_WAKE, 0x7fffffff);
}

static void set_fifo(int prio)
{
    struct sched_param sp = { .sched_priority = prio };
    if (sched_setscheduler(0, SCHED_FIFO, &sp) != 0)
        perror("sched_setscheduler(FIFO)");
}

static int cur_policy(pid_t tid, int *prio)
{
    struct sched_param sp;
    int pol = sched_getscheduler(tid);
    if (pol < 0)
        return -1;
    memset(&sp, 0, sizeof(sp));
    if (sched_getparam(tid, &sp) != 0)
        return -1;
    *prio = sp.sched_priority;
    return pol;
}

struct warg {
    int idx;
    pid_t tid;
    long max_us;
};

static void *worker(void *ap)
{
    struct warg *w = ap;
    const int i = w->idx;
    w->tid = syscall(SYS_gettid);
    if (i == 1)
        set_fifo(40);
    else if (i == 2)
        set_fifo(60);
    else if (i == 3)
        set_fifo(90);

    for (;;) {
        gate_wait(&gate_round, 0); // round start
        if (stop_now)
            return NULL;

        if (i == 0) {
            // T0: own L0 uncontended, hold until released.
            lock_word[0] = (uint32_t)w->tid;
            __sync_synchronize();
            gate_wait(&gate_release, 0);
            futex_op(&lock_word[0], FUTEX_UNLOCK_PI, 0);
        } else if (i < DEPTH) {
            // T1/T2: wait for our tier gate, hold L(i), block on
            // L(i-1); the handoff wakes us OWNING L(i-1).
            gate_wait(&gate_tier[i], 0);
            lock_word[i] = (uint32_t)w->tid;
            __sync_synchronize();
            long rcq = futex_op(&lock_word[i - 1], FUTEX_LOCK_PI, 0);
            if (rcq != 0) printf("T%d-LOCK-ERR rc=%ld w=%x\n", i, rcq, lock_word[i - 1]);
            futex_op(&lock_word[i - 1], FUTEX_UNLOCK_PI, 0);
            futex_op(&lock_word[i], FUTEX_UNLOCK_PI, 0);
        } else {
            // T3: top of the chain — a pure FIFO-90 waiter on L2.
            gate_wait(&gate_tier[DEPTH], 0);
            struct timespec a, b;
            clock_gettime(CLOCK_MONOTONIC, &a);
            long rc3 = futex_op(&lock_word[DEPTH - 1], FUTEX_LOCK_PI, 0);
            if (rc3 != 0) printf("T3-LOCK-ERR rc=%ld w=%x\n", rc3, lock_word[DEPTH - 1]);
            clock_gettime(CLOCK_MONOTONIC, &b);
            long us = (b.tv_sec - a.tv_sec) * 1000000L +
                      (b.tv_nsec - a.tv_nsec) / 1000;
            if (us > w->max_us)
                w->max_us = us;
            futex_op(&lock_word[DEPTH - 1], FUTEX_UNLOCK_PI, 0);
        }

        __sync_synchronize();
        __sync_fetch_and_add(&done_count, 1);
        gate_wait(&gate_round, 1); // round teardown
        __sync_fetch_and_add(&back_count, 1);
    }
    return NULL;
}

/// Bounded userspace spin waiting for a word condition (no syscalls —
/// immune to the nanosleep wake-loss bug; the conditions settle in
/// microseconds to milliseconds).
static int spin_until(volatile uint32_t *addr, uint32_t mask, uint32_t want,
                      long iters)
{
    for (long i = 0; i < iters; i++) {
        if ((*addr & mask) == want)
            return 1;
        sched_yield();
    }
    return (*addr & mask) == want;
}

int main(int argc, char **argv)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    long rounds = argc > 1 ? atol(argv[1]) : 200;

    pthread_t th[DEPTH + 1];
    struct warg wa[DEPTH + 1];
    memset(wa, 0, sizeof(wa));
    for (int i = 0; i < DEPTH + 1; i++) {
        wa[i].idx = i;
        pthread_create(&th[i], NULL, worker, &wa[i]);
    }
    for (int i = 0; i < DEPTH + 1; i++)
        while (wa[i].tid == 0)
            sched_yield();

    long boost_ok = 0, starves = 0, max_wait_us = 0;
    struct timespec rt0, rt1;
    for (long r = 0; r < rounds; r++) {
        clock_gettime(CLOCK_MONOTONIC, &rt0);
        // Reset round state.
        for (int i = 0; i < DEPTH; i++) {
            lock_word[i] = 0;
            gate_tier[i] = 0;
        }
        gate_tier[DEPTH] = 0;
        gate_release = 0;
        done_count = 0;
        back_count = 0;
        __sync_synchronize();
        gate_open(&gate_round, 1);

        // Assemble tier by tier; spin until the expected owner (plus
        // WAITERS for the queued tiers) is in place, then open the
        // next tier. Yield-spin bounds: seconds of wall time — plenty.
        int bad = 0;
        // Deterministic ordering: each lock word must show its owner
        // BEFORE the next tier's gate opens — otherwise a later tier
        // can acquire an earlier lock UNCONTESTED and the chain never
        // forms (the free-acquire races of probe v1/v2).
        if (!spin_until(&lock_word[0], 0xffffffffu, wa[0].tid, 20000000L))
            bad = 1;
        gate_open(&gate_tier[1], 1); // T1 takes L1, blocks on L0
        if (!spin_until(&lock_word[1], 0xffffffffu, wa[1].tid, 20000000L) ||
            !spin_until(&lock_word[0], 0x80000000u, 0x80000000u, 20000000L))
            bad = 1;
        gate_open(&gate_tier[2], 1); // T2 takes L2, blocks on L1
        if (!spin_until(&lock_word[2], 0xffffffffu, wa[2].tid, 20000000L) ||
            !spin_until(&lock_word[1], 0x80000000u, 0x80000000u, 20000000L))
            bad = 1;
        gate_open(&gate_tier[DEPTH], 1); // T3 blocks on L2
        // Wait for the CHAIN TO ARM (not the WAITERS latch — the latch
        // lands before the kernel queues the waiter and walks the
        // boosts; polling the word alone races that window). Poll the
        // actual scheduling outcome: all three holders at FIFO-90.
        {
            long armed = 0;
            for (long i = 0; i < 20000000L; i++) {
                int p0, p1, p2;
                if (cur_policy(wa[0].tid, &p0) == SCHED_FIFO && p0 == 90 &&
                    cur_policy(wa[1].tid, &p1) == SCHED_FIFO && p1 == 90 &&
                    cur_policy(wa[2].tid, &p2) == SCHED_FIFO && p2 == 90) {
                    armed = 1;
                    break;
                }
                sched_yield();
            }
            if (!armed) {
                bad = 2;
                printf("ARM-FAIL r=%ld w2=%x\n", r, lock_word[2]);
            }
        }

        // Full chain is up: T3(90) boosts T2 to 90, T2@90 re-ranks on
        // L1 and boosts T1 to 90, T1@90 re-ranks on L0 and converts T0
        // (CFS) to FIFO 90.
        int ok = !bad;
        int pol, prio;
        static long fails = 0;
        pol = cur_policy(wa[0].tid, &prio);
        if (!(pol == SCHED_FIFO && prio == 90)) {
            ok = 0;
            if (fails < 8) {
                printf("CHAIN-FAIL r=%ld bad=%d T0(CFS) pol=%d prio=%d want FIFO/90 w0=%x w1=%x w2=%x\n",
                        r, bad, pol, prio, lock_word[0], lock_word[1],
                        lock_word[2]);
                fails++;
            }
        }
        for (int t = 1; t < DEPTH; t++) {
            pol = cur_policy(wa[t].tid, &prio);
            if (!(pol == SCHED_FIFO && prio == 90)) {
                ok = 0;
                if (fails < 8) {
                    printf("CHAIN-FAIL r=%ld bad=%d T%d pol=%d prio=%d want FIFO/90 w0=%x w1=%x w2=%x\n",
                            r, bad, t, pol, prio, lock_word[0], lock_word[1],
                            lock_word[2]);
                    fails++;
                }
            }
        }
        if (ok)
            boost_ok++;
        else if (boost_ok + (r - boost_ok) > 0) {
            static long first_fail = -1;
            if (first_fail < 0)
                first_fail = r;
            if (r > first_fail + 2)
                goto out;
        }

        // Release the cascade and drain (yield-poll, no timer sleep).
        gate_open(&gate_release, 1);
        long spins = 0;
        while (done_count < DEPTH + 1) {
            sched_yield();
            if (++spins > 20000000L) { // ~seconds of yielding
                starves++;
                for (int t = 0; t < DEPTH + 1; t++) {
                    int p2 = -9, pr2 = -9;
                    struct sched_param ssp;
                    p2 = sched_getscheduler(wa[t].tid);
                    memset(&ssp, 0, sizeof(ssp));
                    sched_getparam(wa[t].tid, &ssp);
                    pr2 = ssp.sched_priority;
                    char path[64], st = '?';
                    snprintf(path, sizeof(path), "/proc/%d/stat", wa[t].tid);
                    FILE *f = fopen(path, "r");
                    if (f) {
                        char buf[256];
                        if (fgets(buf, sizeof(buf), f)) {
                            char *p1 = strrchr(buf, '(');
                            char *p2c = p1 ? strchr(p1, ')') : NULL;
                            if (p2c && p2c[1] == ' ')
                                st = p2c[2];
                        }
                        fclose(f);
                    }
                    fprintf(stderr,
                            "STUCK r=%ld T%d tid=%d pol=%d prio=%d state=%c done=%u\n",
                            r, t, wa[t].tid, p2, pr2, st, done_count);
                }
                break;
            }
        }
        if (wa[DEPTH].max_us > max_wait_us)
            max_wait_us = wa[DEPTH].max_us;
        gate_open(&gate_round, 0); // release workers back to the gate
        while (back_count < DEPTH + 1)
            sched_yield();
        (void)rt0; (void)rt1;
    }
    // Stop: open the gates to a value != the expected one so every
    // gate_wait returns (0 would re-sleep forever — the join deadlock
    // of probe v3).
out:
    stop_now = 1;
    gate_open(&gate_round, 2);
    gate_open(&gate_release, 2);
    for (int i = 0; i < DEPTH + 1; i++)
        pthread_join(th[i], NULL);

    int pass = (boost_ok == rounds) && (starves == 0);
    printf("RES|pi_chain|%s|rounds=%ld boost_ok=%ld starved=%ld max_wait_us=%ld\n",
           pass ? "PASS" : "FAIL", rounds, boost_ok, starves, max_wait_us);
    return pass ? 0 : 1;
}
