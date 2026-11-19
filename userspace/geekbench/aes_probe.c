/* aes_probe.c — AES-XTS diagnosis probe for the Geekbench rig (PID 1 or
 * standalone). Answers, from inside the measured environment:
 *
 *   1. What CPUID/xgetbv the process sees (AES-NI bit 25, OSXSAVE, AVX2,
 *      AVX-512, VAES) — the dispatch inputs OpenSSL/Glekbench read.
 *   2. Whether `aesenc` (SSE-domain AES-NI) executes at all (SIGILL
 *      guard) and its raw issue rate (compute-only, L1-resident).
 *   3. Streaming memory bandwidth over 256 MB (>> L3): first-touch
 *      (fault-in, with ru_minflt), then established-page read/write
 *      passes, single- and 4-thread — separates the memory path from
 *      the crypto primitive.
 *   4. malloc/munmap 128 MB cycles — does each cycle re-fault (glibc
 *      mmap threshold behavior under this kernel).
 *   5. Preemption interference: voluntary/involuntary context switches
 *      during a 2 s userspace spin.
 *   6. clock_gettime cost (syscall vs vDSO presence).
 *   7. An AES-XTS-shaped pass: 16-byte-stride aesenc walk over 128 MB —
 *      the GB5 workload's access pattern in miniature.
 *
 * Static musl build, runs natively and as Rux init:
 *   musl-gcc -static -O2 -o aes_probe aes_probe.c -lpthread
 * Every result line is `PROBE <key> <value>` for easy extraction.
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <signal.h>
#include <setjmp.h>
#include <time.h>
#include <errno.h>
#include <sys/resource.h>
#include <sys/time.h>
#include <pthread.h>

static double now_s(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec * 1e-9;
}

static long minor_faults(void)
{
    struct rusage ru;
    getrusage(RUSAGE_SELF, &ru);
    return ru.ru_minflt;
}

/* ---- SIGILL guard ---- */
static sigjmp_buf ill_jmp;
static void ill_handler(int sig) { (void)sig; siglongjmp(ill_jmp, 1); }

static void cpuid_(unsigned leaf, unsigned sub, unsigned *a, unsigned *b,
                   unsigned *c, unsigned *d)
{
    __asm__ volatile("cpuid"
                     : "=a"(*a), "=b"(*b), "=c"(*c), "=d"(*d)
                     : "a"(leaf), "c"(sub));
}

/* ---- 1. CPUID / xgetbv ---- */
static void report_cpu(void)
{
    unsigned a, b, c, d;
    cpuid_(1, 0, &a, &b, &c, &d);
    printf("PROBE cpuid1_ecx %#x (aesni=%d xsave=%d osxsave=%d avx=%d rdrand=%d)\n",
           c, !!(c & (1 << 25)), !!(c & (1 << 26)), !!(c & (1 << 27)),
           !!(c & (1 << 28)), !!(c & (1 << 30)));
    cpuid_(7, 0, &a, &b, &c, &d);
    printf("PROBE cpuid7_ebx %#x (avx2=%d avx512f=%d bmi2=%d shstk=%d)\n",
           b, !!(b & (1 << 5)), !!(b & (1 << 16)), !!(b & (1 << 8)),
           !!(b & (1 << 7)));
    printf("PROBE cpuid7_ecx %#x (vaes=%d avx512vl=%d gfni=%d)\n",
           c, !!(c & (1 << 9)), !!(c & (1 << 31)), !!(c & (1 << 8)));
    /* xgetbv — OSXSAVE is on under both the reference kernel and Rux */
    unsigned long long xcr0 = 0;
    __asm__ volatile("xgetbv" : "=A"(xcr0) : "c"(0));
    printf("PROBE xcr0 %#llx (x87=%d sse=%d avx=%d opmask=%d zmm_hi=%d hi16_zmm=%d)\n",
           xcr0, !!(xcr0 & 1), !!(xcr0 & 2), !!(xcr0 & 4), !!(xcr0 & 32),
           !!(xcr0 & 64), !!(xcr0 & 128));
}

/* ---- 2. aesenc functional + throughput ---- */
static void probe_aesni(void)
{
    struct sigaction sa, old;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = ill_handler;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGILL, &sa, &old);

    volatile int ok = 0;
    static unsigned char buf[32] __attribute__((aligned(16)));
    if (sigsetjmp(ill_jmp, 1) == 0) {
        __asm__ volatile(
            "movdqa (%0), %%xmm0\n\t"
            "movdqa 16(%0), %%xmm1\n\t"
            "aesenc %%xmm1, %%xmm0\n\t"
            "movdqa %%xmm0, (%0)"
            :
            : "r"(buf)
            : "xmm0", "xmm1", "memory");
        ok = 1;
    }
    sigaction(SIGILL, &old, NULL);
    printf("PROBE aesenc_exec %s\n", ok ? "OK" : "SIGILL");

    if (!ok)
        return;

    /* Throughput: 8-way independent aesenc chains, L1-resident. */
    static const unsigned int one[4] __attribute__((aligned(16))) = {1, 1, 1, 1};
    const long rounds = 200000000L; /* 200M rounds of 8 chains */
    register long cnt = rounds;
    double t0 = now_s();
    __asm__ volatile(
        "pxor %%xmm0, %%xmm0\n\t"
        "movdqa (%1), %%xmm8\n\t"
        "movdqa %%xmm8, %%xmm1\n\t"
        "movdqa %%xmm8, %%xmm2\n\t"
        "movdqa %%xmm8, %%xmm3\n\t"
        "movdqa %%xmm8, %%xmm4\n\t"
        "movdqa %%xmm8, %%xmm5\n\t"
        "movdqa %%xmm8, %%xmm6\n\t"
        "movdqa %%xmm8, %%xmm7\n\t"
        "1:\n\t"
        "aesenc %%xmm8, %%xmm0\n\t"
        "aesenc %%xmm8, %%xmm1\n\t"
        "aesenc %%xmm8, %%xmm2\n\t"
        "aesenc %%xmm8, %%xmm3\n\t"
        "aesenc %%xmm8, %%xmm4\n\t"
        "aesenc %%xmm8, %%xmm5\n\t"
        "aesenc %%xmm8, %%xmm6\n\t"
        "aesenc %%xmm8, %%xmm7\n\t"
        "sub $1, %0\n\t"
        "jnz 1b\n\t"
        : "+r"(cnt)
        : "r"(one)
        : "xmm0", "xmm1", "xmm2", "xmm3", "xmm4", "xmm5", "xmm6", "xmm7",
          "xmm8");
    double dt = now_s() - t0;
    printf("PROBE aesenc_mrounds_per_s %.2f (8-chain, %.3f s)\n",
           (double)rounds / dt / 1e6, dt);
}

/* ---- 3. streaming memory ---- */
#define STREAM_MB 256
#define STREAM_BYTES ((size_t)STREAM_MB * 1024 * 1024)

static unsigned char *sbuf;

static void touch_pages(unsigned char *p, size_t n)
{
    for (size_t i = 0; i < n; i += 4096)
        p[i] = (unsigned char)(i >> 12);
}

static double stream_read_once(unsigned char *p, size_t n)
{
    unsigned long acc = 0;
    double t0 = now_s();
    for (size_t i = 0; i + 64 <= n; i += 64)
        acc += p[i];
    double dt = now_s() - t0;
    /* keep acc alive */
    if (acc == 0xdeadbeefUL)
        printf(".");
    return dt;
}

static void *thread_read(void *arg)
{
    unsigned char *p = (unsigned char *)arg;
    unsigned long acc = 0;
    for (size_t i = 0; i + 64 <= STREAM_BYTES / 4; i += 64)
        acc += p[i];
    if (acc == 0xdeadbeefUL)
        printf(".");
    return NULL;
}

static void probe_stream(void)
{
    sbuf = malloc(STREAM_BYTES);
    if (!sbuf) {
        printf("PROBE stream_alloc FAIL\n");
        return;
    }
    long f0 = minor_faults();
    double t0 = now_s();
    touch_pages(sbuf, STREAM_BYTES);
    double dt = now_s() - t0;
    printf("PROBE stream_firsttouch %.2f GB/s (%.3f s, %ld minflt, %.1f us/4K)\n",
           STREAM_BYTES / 1e9 / dt, dt, minor_faults() - f0,
           dt * 1e6 / (STREAM_BYTES / 4096.0));

    for (int pass = 1; pass <= 3; pass++) {
        double d = stream_read_once(sbuf, STREAM_BYTES);
        printf("PROBE stream_read_pass%d %.2f GB/s\n", pass,
               STREAM_BYTES / 1e9 / d);
    }
    for (int pass = 1; pass <= 2; pass++) {
        double tp = now_s();
        memset(sbuf, pass, STREAM_BYTES);
        double dp = now_s() - tp;
        printf("PROBE stream_write_pass%d %.2f GB/s\n", pass,
               STREAM_BYTES / 1e9 / dp);
    }

    /* 4-thread read */
    pthread_t th[4];
    double t0b = now_s();
    for (int i = 0; i < 4; i++)
        pthread_create(&th[i], NULL, thread_read, sbuf + i * (STREAM_BYTES / 4));
    for (int i = 0; i < 4; i++)
        pthread_join(th[i], NULL);
    double dtb = now_s() - t0b;
    printf("PROBE stream_read_4thr %.2f GB/s aggregate\n",
           STREAM_BYTES / 1e9 / dtb);
}

/* ---- 4. malloc/munmap 128 MB cycle ---- */
static void probe_realloc(void)
{
    for (int i = 1; i <= 3; i++) {
        long f0 = minor_faults();
        double t0 = now_s();
        unsigned char *p = malloc(128u << 20);
        touch_pages(p, 128u << 20);
        p[0]++;
        free(p);
        double dt = now_s() - t0;
        printf("PROBE malloc_cycle%d %.3f s %ld minflt\n", i, dt,
               minor_faults() - f0);
    }
}

/* ---- 5. preemption during spin ---- */
static void probe_preempt(void)
{
    struct rusage ru0, ru1;
    getrusage(RUSAGE_SELF, &ru0);
    double t0 = now_s();
    volatile unsigned long x = 0;
    while (now_s() - t0 < 2.0)
        x++;
    getrusage(RUSAGE_SELF, &ru1);
    printf("PROBE spin2s nvcsw %ld nivcsw %ld (x=%lu)\n",
           ru1.ru_nvcsw - ru0.ru_nvcsw, ru1.ru_nivcsw - ru0.ru_nivcsw, x);
}

/* ---- 6. clock_gettime cost ---- */
static void probe_clock(void)
{
    const int n = 2000000;
    struct timespec ts;
    double t0 = now_s();
    for (int i = 0; i < n; i++)
        clock_gettime(CLOCK_MONOTONIC, &ts);
    double dt = now_s() - t0;
    printf("PROBE clock_gettime_ns_per_op %.1f\n", dt * 1e9 / n);
}

/* ---- 7. AES-XTS-shaped pass: aesenc walk over 128 MB ---- */
static void probe_xts_walk(void)
{
    const size_t n = 128u << 20;
    unsigned char *p = malloc(n);
    if (!p)
        return;
    touch_pages(p, n); /* fault in first, so passes measure steady state */
    for (int pass = 1; pass <= 4; pass++) {
        double t0 = now_s();
        unsigned char *q = p;
        __asm__ volatile(
            "pxor %%xmm0, %%xmm0\n\t"
            "mov $1, %%eax\n\t"
            "movd %%eax, %%xmm8\n\t"
            "pshufd $0, %%xmm8, %%xmm8\n\t"
            "mov %1, %%rcx\n\t"
            "1:\n\t"
            "movdqu (%0), %%xmm1\n\t"
            "pxor %%xmm8, %%xmm1\n\t"
            "aesenc %%xmm1, %%xmm0\n\t"
            "movdqu %%xmm0, (%0)\n\t"
            "add $16, %0\n\t"
            "sub $16, %%rcx\n\t"
            "jnz 1b\n\t"
            : "+r"(q)
            : "r"(n)
            : "xmm0", "xmm1", "xmm8", "rax", "rcx", "memory");
        double dt = now_s() - t0;
        printf("PROBE xts_walk128M_pass%d %.2f GB/s (%.3f s)\n", pass,
               n / 1e9 / dt, dt);
    }
    free(p);
}

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("PROBE start\n");
    report_cpu();
    probe_aesni();
    probe_stream();
    probe_realloc();
    probe_preempt();
    probe_clock();
    probe_xts_walk();
    printf("PROBE done\n");
    return 0;
}
