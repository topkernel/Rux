/* probe_open.c — open/close vs stat cost decomposition, same boot.
 *
 * Discriminates three measurement shapes on the same file:
 *   (a) per-sample rdtsc deltas  -> median/mean/p99/max distribution
 *   (b) plain wall-clock loop    -> the lmbench benchmp shape (no
 *       per-iter sampling; amortizes async stealing into the mean)
 *   (c) stat on the same file    -> walk+entry reference point
 *
 * Usage: probe_open [file ...]   (default: /usr/include/linux/types.h
 * plus /tmp/probe.tmp, created if missing)
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <time.h>

typedef unsigned long long u64;
typedef long long s64;

static inline u64 rdtsc_(void)
{
    unsigned lo, hi;
    __asm__ __volatile__("rdtsc" : "=a"(lo), "=d"(hi));
    return ((u64)hi << 32) | lo;
}

static s64 mono_ns(void)
{
    struct timespec ts;
    syscall(SYS_clock_gettime, 4, &ts);
    return (s64)ts.tv_sec * 1000000000LL + (s64)ts.tv_nsec;
}

static double g_hz;

static void calibrate(void)
{
    s64 t0 = mono_ns();
    u64 c0 = rdtsc_();
    struct timespec req = { 0, 200 * 1000 * 1000 };
    nanosleep(&req, NULL);
    u64 c1 = rdtsc_();
    s64 t1 = mono_ns();
    g_hz = (double)(c1 - c0) / (double)(t1 - t0); /* cycles per ns */
    printf("probe: tsc %.0f MHz\n", g_hz * 1000.0);
}

static int cmp_u64(const void *a, const void *b)
{
    u64 x = *(const u64 *)a, y = *(const u64 *)b;
    return x < y ? -1 : x > y ? 1 : 0;
}

#define N_SAMPLE 200000
static u64 *g_samples;

/* (a) sampled distribution */
static void sampled(const char *label, void (*op)(const char *), const char *file)
{
    for (int i = 0; i < 2000; i++) /* warmup */
        op(file);
    for (int i = 0; i < N_SAMPLE; i++) {
        u64 t0 = rdtsc_();
        op(file);
        u64 t1 = rdtsc_();
        g_samples[i] = t1 - t0;
    }
    qsort(g_samples, N_SAMPLE, sizeof(u64), cmp_u64);
    u64 sum = 0;
    for (int i = 0; i < N_SAMPLE; i++)
        sum += g_samples[i];
    printf("%s[%s] n=%d min=%.0f p10=%.0f med=%.0f mean=%.0f p90=%.0f p99=%.0f p999=%.0f max=%.0f ns\n",
           label, file, N_SAMPLE,
           g_samples[0] / g_hz,
           g_samples[N_SAMPLE / 10] / g_hz,
           g_samples[N_SAMPLE / 2] / g_hz,
           (double)sum / N_SAMPLE / g_hz,
           g_samples[N_SAMPLE * 9 / 10] / g_hz,
           g_samples[N_SAMPLE - N_SAMPLE / 100] / g_hz,
           g_samples[N_SAMPLE - N_SAMPLE / 1000] / g_hz,
           g_samples[N_SAMPLE - 1] / g_hz);
    /* top-5 outliers in ns */
    printf("%s[%s] top5: %.0f %.0f %.0f %.0f %.0f ns\n", label, file,
           g_samples[N_SAMPLE - 1] / g_hz,
           g_samples[N_SAMPLE - 2] / g_hz,
           g_samples[N_SAMPLE - 3] / g_hz,
           g_samples[N_SAMPLE - 4] / g_hz,
           g_samples[N_SAMPLE - 5] / g_hz);
}

/* stall hunt: split open/close timing, timestamp every >100us sample.
 * Reports whether the big stalls sit in open() or close() and whether
 * they are wall-clock periodic (delta ~ one tick) or iteration-driven. */
#define STALL_NS 100000
static void stall_hunt(const char *file)
{
    int n_stall = 0;
    s64 prev_stall_ns = 0;
    /* second pass buffer: deltas + timestamps for the first 40 stalls */
    for (int i = 0; i < N_SAMPLE; i++) {
        u64 t0 = rdtsc_();
        int fd = open(file, 0);
        u64 t1 = rdtsc_();
        if (fd >= 0)
            close(fd);
        u64 t2 = rdtsc_();
        double d_open = (double)(t1 - t0) / g_hz;
        double d_close = (double)(t2 - t1) / g_hz;
        if (d_open > STALL_NS || d_close > STALL_NS) {
            s64 now = mono_ns();
            printf("stall[%s] #%d open=%.0f close=%.0f at=%ld since_prev=%ld ns\n",
                   file, n_stall, d_open, d_close, (long)now,
                   (long)(prev_stall_ns ? now - prev_stall_ns : -1));
            prev_stall_ns = now;
            if (++n_stall >= 40)
                break;
        }
    }
    if (!n_stall)
        printf("stall[%s] none >%dns in %d iters\n", file, STALL_NS, N_SAMPLE);
}

/* (b) plain wall loop, lmbench shape */
static void wall_loop(const char *label, void (*op)(const char *), const char *file)
{
    const int n = 2000000;
    for (int i = 0; i < 50000; i++)
        op(file);
    s64 t0 = mono_ns();
    for (int i = 0; i < n; i++)
        op(file);
    s64 t1 = mono_ns();
    printf("%s-wall[%s] n=%d mean=%.1f ns/iter\n", label, file, n,
           (double)(t1 - t0) / n);
}

static void op_openclose(const char *f)
{
    int fd = open(f, 0);
    if (fd >= 0)
        close(fd);
}

static void op_stat(const char *f)
{
    struct stat sb;
    (void)stat(f, &sb);
}

int main(int argc, char **argv)
{
    g_samples = malloc(N_SAMPLE * sizeof(u64));
    if (!g_samples) {
        printf("probe: oom\n");
        return 1;
    }
    calibrate();

    const char *files[4];
    int nf = 0;
    files[nf++] = "/usr/include/linux/types.h";
    files[nf++] = "/tmp/probe.tmp";
    for (int i = 1; i < argc && nf < 4; i++)
        files[nf++] = argv[i];

    /* make sure /tmp/probe.tmp exists (open with flags 0 = O_RDONLY on
     * an existing file; missing file aborts the lmbench case) */
    int tf = open("/tmp/probe.tmp", O_CREAT | O_TRUNC | O_WRONLY, 0644);
    if (tf >= 0) {
        if (write(tf, "x", 1) != 1)
            /* ignore */;
        close(tf);
    }

    for (int i = 0; i < nf; i++) {
        struct stat st;
        if (stat(files[i], &st) < 0) {
            printf("probe: skip missing %s\n", files[i]);
            continue;
        }
        sampled("openclose", op_openclose, files[i]);
        wall_loop("openclose", op_openclose, files[i]);
        stall_hunt(files[i]);
        sampled("stat", op_stat, files[i]);
        wall_loop("stat", op_stat, files[i]);
    }
    printf("PROBE DONE\n");
    fflush(stdout);
    return 0;
}
