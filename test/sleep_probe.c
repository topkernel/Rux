/*
 * sleep_probe.c — timer-precision oracle for the vsync starvation hunt
 * (compose-content). The vendor DrmVsyncWorker paces display vsync with
 * usleep(16000) + clock_gettime(MONOTONIC); on the real boot the whole
 * graphics pipeline ticks at ~1 frame per many seconds instead of 60Hz.
 * This probe measures what a 16ms sleep actually costs on Rux:
 *
 *   - usleep(16000) x N: per-iteration and total wall time
 *   - nanosleep(16ms) direct
 *   - clock_getres(CLOCK_MONOTONIC)
 *   - futex-free; single thread
 *
 * Verdict: SLEEP_PROBE RESULT: avg=<us> max=<us> (ok if avg ~16000).
 */
#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static unsigned long long ts_ns(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (unsigned long long)ts.tv_sec * 1000000000ULL + ts.tv_nsec;
}

int main(int argc, char **argv)
{
    int spinners = 0;
    if (argc > 1)
        spinners = atoi(argv[1]);
    printf("SLEEP_PROBE: start spinners=%d\r\n", spinners);
    for (int i = 0; i < spinners; i++) {
        if (fork() == 0) {
            volatile unsigned long x = 0;
            for (;;)
                x += 1;
        }
    }

    struct timespec res;
    clock_getres(CLOCK_MONOTONIC, &res);
    printf("SLEEP_PROBE: clock_res %ld.%09ld\r\n",
           (long)res.tv_sec, res.tv_nsec);

    const int N = 30;
    unsigned long long total = 0, mx = 0, mn = ~0ULL;
    for (int i = 0; i < N; i++) {
        unsigned long long t0 = ts_ns();
        usleep(16000);
        unsigned long long d = ts_ns() - t0;
        total += d;
        if (d > mx)
            mx = d;
        if (d < mn)
            mn = d;
        if (i < 5 || d > 1000000)
            printf("SLEEP_PROBE: usleep#%d %llu us\r\n", i, d / 1000);
    }
    printf("SLEEP_PROBE: usleep avg=%llu min=%llu max=%llu (want ~16000)\r\n",
           total / N / 1000, mn / 1000, mx / 1000);

    /* nanosleep direct, 10 iters */
    struct timespec req = { .tv_sec = 0, .tv_nsec = 16000000 };
    unsigned long long t0 = ts_ns();
    for (int i = 0; i < 10; i++) {
        struct timespec rem;
        while (nanosleep(&req, &rem) != 0 && errno == EINTR)
            req = rem;
    }
    printf("SLEEP_PROBE: nanosleep avg=%llu us over 10\r\n",
           (ts_ns() - t0) / 10 / 1000);

    printf("SLEEP_PROBE RESULT: avg=%llu max=%llu\r\n",
           total / N / 1000, mx / 1000);
    for (;;)
        pause();
}
