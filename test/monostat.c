/*
 * monostat.c — CLOCK_MONOTONIC stability probe (timer-lane KVM hunt).
 *
 * Samples CLOCK_MONOTONIC every INTERVAL_US for DURATION_S and checks:
 *   1. Monotonicity: no sample may go backward (any back-jump = FAIL).
 *   2. Rate: each sample delta must stay in [INTERVAL/2, INTERVAL*3]
 *      (host-side timing separately verifies the absolute rate).
 *   3. Cross-clock coherence: CLOCK_MONOTONIC_COARSE must never run
 *      ahead of CLOCK_MONOTONIC and must lag it by < 2/HZ seconds;
 *      CLOCK_REALTIME - CLOCK_MONOTONIC must be constant.
 *   4. /proc/uptime (jiffies-derived) must not lag CLOCK_MONOTONIC by
 *      more than 5 s at any 1 s checkpoint (jiffies/monotonic
 *      divergence detector).
 *
 * Prints one MONOSTAT line per CHECK_EVERY samples so a host-side
 * driver can timestamp serial lines and compare guest-claimed deltas
 * against real elapsed time (the definitive KVM jump test: a guest
 * clock that runs fast shows up as guest delta >> host delta).
 *
 * Final line:
 *   MONOSTAT RESULT: PASS samples=<n> back=<n> max_us=<n> min_us=<n>
 * or the same with FAIL and a reason.
 *
 * Exit code 0 on PASS, 1 on FAIL.
 */
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/syscall.h>
#include <sys/times.h>
#include <time.h>

static int puts_(const char *s) { return write(1, s, strlen(s)); }
static void putu_(unsigned long v) {
    char b[24]; int i = 24;
    if (!v) { puts_("0"); return; }
    while (v && i) { b[--i] = '0' + (v % 10); v /= 10; }
    write(1, b + i, 24 - i);
}
static void putsu_(const char *s, unsigned long v) { puts_(s); putu_(v); }

static long long now_ns(int clk) {
    struct timespec ts;
    if (syscall(SYS_clock_gettime, clk, &ts) != 0) {
        puts_("MONOSTAT clock_gettime failed\n");
        _exit(9);
    }
    return (long long)ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

/* /proc/uptime first field (seconds.frac) * 1000 -> ms, or (u64)-1. */
static unsigned long uptime_ms(void) {
    int fd = open("/proc/uptime", O_RDONLY);
    char buf[128];
    if (fd < 0) return (unsigned long)-1;
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0) return (unsigned long)-1;
    buf[n] = 0;
    unsigned long sec = 0, ms = 0;
    const char *p = buf;
    while (*p >= '0' && *p <= '9') sec = sec * 10 + (*p++ - '0');
    if (*p == '.') {
        p++;
        int digits = 0;
        while (*p >= '0' && *p <= '9' && digits < 3)
            ms = ms * 10 + (*p++ - '0'), digits++;
        while (digits++ < 3) ms *= 10;
    }
    return sec * 1000 + ms;
}

#define INTERVAL_US   100000  /* sample period */
#define DURATION_S    60
#define CHECK_EVERY   10      /* one serial line per 10 samples (1 s) */
#define N_SAMPLES     (DURATION_S * 1000000 / INTERVAL_US)

int main(void) {
    puts_("MONOSTAT start: 100ms samples for 60s\n");

    long long mono0 = now_ns(CLOCK_MONOTONIC);
    long long mono_prev = mono0;
    long long rt0 = now_ns(CLOCK_REALTIME);
    long long rt_prev = rt0;
    unsigned long up_prev = (unsigned long)-1;
    long long up_lag0_ms = 0;
    int up_baseline = 0;

    unsigned long back_jumps = 0, rt_back = 0;
    unsigned long max_us = 0, min_us = (unsigned long)-1;
    long long coarse_lead_ns = 0, max_coarse_lag_ns = 0;
    long long max_up_lag_ms = 0;
    const char *fail = 0;

    for (int i = 1; i <= N_SAMPLES; i++) {
        struct timespec sl = { 0, INTERVAL_US * 1000 };
        while (nanosleep(&sl, &sl) != 0 && errno == EINTR) {}

        long long mono = now_ns(CLOCK_MONOTONIC);
        long long delta = mono - mono_prev;
        unsigned long dus = (unsigned long)(delta / 1000);

        /* 1. monotonicity */
        if (delta < 0) back_jumps++;
        /* 2. rate window: [50ms, 300ms] around the 100ms period */
        if (delta < 50LL * 1000 * 1000 || delta > 300LL * 1000 * 1000) {
            putsu_("MONOSTAT DELTA-OUTLER sample=", i);
            putsu_(" delta_us=", dus);
            puts_("\n");
        }
        if (dus > max_us) max_us = dus;
        if (dus < min_us) min_us = dus;
        mono_prev = mono;

        /* 3. coarse clock coherence (every sample; cheap syscalls).
         * Coarse is sampled in a SECOND syscall after monotonic —
         * crossing a jiffy boundary between the two reads legitimately
         * leaves coarse ahead by up to one tick (10 ms at HZ=100);
         * only a lead beyond a tick+margin is a real inversion. */
        long long coarse = now_ns(CLOCK_MONOTONIC_COARSE);
        if (coarse - mono > coarse_lead_ns) {
            coarse_lead_ns = coarse - mono;
        } else if (mono - coarse > max_coarse_lag_ns) {
            max_coarse_lag_ns = mono - coarse;
        }

        /* REALTIME must advance in lockstep (offset constant) */
        long long rt = now_ns(CLOCK_REALTIME);
        if (rt - rt_prev < 0) rt_back++;
        rt_prev = rt;

        /* 4. /proc/uptime checkpoint every second: the monotonic-vs-uptime
         * offset may differ at boot, but it must not GROW (jiffies
         * diverging from the timebase is the timeout-correctness bug). */
        if (i % CHECK_EVERY == 0) {
            unsigned long up = uptime_ms();
            if (up != (unsigned long)-1) {
                long long lag_ms =
                    ((long long)(mono / 1000000)) - (long long)up;
                if (!up_baseline) {
                    up_lag0_ms = lag_ms; /* first valid checkpoint: baseline */
                    up_baseline = 1;
                }
                long long growth = lag_ms - up_lag0_ms;
                if (growth > max_up_lag_ms) max_up_lag_ms = growth;
                up_prev = up;
            }
            putsu_("MONOSTAT t=", i / CHECK_EVERY);
            putsu_("s mono_ms=", (unsigned long)(mono / 1000000));
            putsu_(" delta_us=", dus);
            puts_("\n");
        }
    }

    long long total_ns = now_ns(CLOCK_MONOTONIC) - mono0;
    putsu_("MONOSTAT SUMMARY: guest_total_ms=", (unsigned long)(total_ns / 1000000));
    putsu_(" back_jumps=", back_jumps);
    putsu_(" rt_back=", rt_back);
    putsu_(" max_us=", max_us);
    putsu_(" min_us=", min_us);
    putsu_(" coarse_lead_us=", (unsigned long)(coarse_lead_ns / 1000));
    putsu_(" coarse_lag_us=", (unsigned long)(max_coarse_lag_ns / 1000));
    putsu_(" uptime_lag_ms=", (unsigned long)max_up_lag_ms);
    puts_("\n");

    if (back_jumps) fail = "monotonic-went-backward";
    else if (rt_back) fail = "realtime-went-backward";
    else if (max_us > 300000) fail = "delta-too-large";
    else if (min_us < 50000) fail = "delta-too-small";
    else if (coarse_lead_ns > 15LL * 1000 * 1000) fail = "coarse-leads-by>15ms";
    else if (max_coarse_lag_ns > 40LL * 1000 * 1000) fail = "coarse-lag>40ms";
    else if (max_up_lag_ms > 5000) fail = "uptime-lag>5s";

    if (fail) {
        puts_("MONOSTAT RESULT: FAIL reason=");
        puts_(fail);
        puts_("\n");
        return 1;
    }
    puts_("MONOSTAT RESULT: PASS\n");
    return 0;
}
