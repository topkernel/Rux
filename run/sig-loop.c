/*
 * sig-loop.c — aarch64 signal-delivery probe (aa64-signal lane).
 *
 * Static, freestanding-ish: raw syscalls only (no libc setup, no
 * restorer passed to rt_sigaction — that exercises the kernel
 * SIGTRAMP page path; -DUSE_RESTORER builds the SA_RESTORER variant
 * glibc/musl use with their own `mov x8,#139; svc #0` stub).
 *
 * Loop: raise(SIGUSR1) via kill(getpid(), 10) → handler increments a
 * counter and returns → after N rounds print the counter through
 * write(1, ...) and exit(0). A SIGSEGV/fault or a wrong count means
 * the frame setup/restore pair is broken.
 *
 * Build:
 *   aarch64-linux-gnu-gcc -static -nostdlib -O1 -o sig-loop sig-loop.c
 *   (+ -DUSE_RESTORER for the restorer variant)
 */
#include <stdint.h>

#define __NR_write 64
#define __NR_exit 93
#define __NR_rt_sigaction 134
#define __NR_kill 129
#define __NR_getpid 172

static long sys(long n, long a, long b, long c, long d)
{
    register long x8 __asm__("x8") = n;
    register long x0 __asm__("x0") = a;
    register long x1 __asm__("x1") = b;
    register long x2 __asm__("x2") = c;
    register long x3 __asm__("x3") = d;
    __asm__ volatile("svc #0"
                     : "+r"(x0)
                     : "r"(x8), "r"(x1), "r"(x2), "r"(x3)
                     : "memory", "cc");
    return x0;
}

static volatile uint64_t g_count;
static volatile uint64_t g_last_sp;
static volatile uint64_t g_v0_probe; /* FP state survives handler runs */

#define SIGUSR1 10

#ifdef USE_RESTORER
extern void sig_restorer(void);
__asm__(".global sig_restorer\n"
        "sig_restorer:\n"
        "  mov x8, #139\n"
        "  svc #0\n");
#endif

static void handler(int sig)
{
    (void)sig;
    /* probe: my SP differs from the interrupted loop's SP — proves a
     * real frame switch happened, and that returning lands back. */
    uint64_t sp;
    __asm__ volatile("mov %0, sp" : "=r"(sp));
    g_last_sp = sp;
    /* FP touch inside the handler: the fpsimd record must put the
     * interrupted v0 back on sigreturn. */
    __asm__ volatile(
        "movi v0.16b, #7\n"
        "mov %0, v0.d[0]\n"
        : "=r"(g_v0_probe));
    g_count++;
}

struct kernel_sigaction {
    void *handler;
    uint64_t flags;
    void *restorer;
    uint64_t mask;
};

#define SA_RESTORER 0x04000000

static void puts(const char *s)
{
    unsigned long n = 0;
    while (s[n])
        n++;
    sys(__NR_write, 1, (long)s, n, 0);
}

static void print_u64(uint64_t v)
{
    char buf[24];
    int i = 23;
    buf[i--] = '\n';
    if (v == 0)
        buf[i--] = '0';
    while (v) {
        buf[i--] = '0' + (v % 10);
        v /= 10;
    }
    sys(__NR_write, 1, (long)(buf + i + 1), 23 - i - 1 + 1, 0);
}

void _start(void)
{
    /* v0 = 0x1111... before any handler runs; the handler sets 0x0707...
     * sigreturn must restore the 0x1111... pattern each round. */
    __asm__ volatile("movi v0.16b, #17");

    struct kernel_sigaction sa;
    sa.handler = handler;
#ifdef USE_RESTORER
    sa.flags = SA_RESTORER;
    sa.restorer = sig_restorer;
#else
    sa.flags = 0; /* no SA_RESTORER: the kernel SIGTRAMP page is the
                   * only return path */
    sa.restorer = 0;
#endif
    sa.mask = 0;
    long rc = sys(__NR_rt_sigaction, SIGUSR1, (long)&sa, 0, 8);
    if (rc != 0) {
        puts("sig-loop: rt_sigaction failed\n");
        sys(__NR_exit, rc, 0, 0, 0);
    }

    long pid = sys(__NR_getpid, 0, 0, 0, 0);
    long kill_rc0 = sys(__NR_kill, pid, SIGUSR1, 0, 0);
    const int rounds = 200;
    uint64_t v0_before_round;
    long kill_rc_min = kill_rc0, kill_rc_max = kill_rc0;
    for (int i = 1; i < rounds; i++) {
        __asm__ volatile("mov %0, v0.d[0]" : "=r"(v0_before_round));
        if (v0_before_round != 0x1111111111111111ULL) {
            puts("sig-loop: FP STATE CORRUPTED after handler\n");
            sys(__NR_exit, 99, 0, 0, 0);
        }
        long k = sys(__NR_kill, pid, SIGUSR1, 0, 0);
        if (k < kill_rc_min) kill_rc_min = k;
        if (k > kill_rc_max) kill_rc_max = k;
    }

    /* ONE write for the whole verdict: the console write path
     * currently livelocks in ERESTARTSYS after its buffer fills
     * (broken-IRQ lane dependency), so every byte must go out in a
     * single drain-able call. */
    {
        static char rep[96];
        int n = 0;
        const char *p;
        for (p = "sig-loop: count="; *p; p++) rep[n++] = *p;
        if (g_count == 0) rep[n++] = '0';
        else {
            char tmp[20];
            int t = 0;
            uint64_t v = g_count;
            while (v) { tmp[t++] = '0' + (v % 10); v /= 10; }
            while (t) rep[n++] = tmp[--t];
        }
        for (p = " sp="; *p; p++) rep[n++] = *p;
        {
            char tmp[17];
            uint64_t v = g_last_sp;
            for (int k = 15; k >= 0; k--) { int d = v & 0xF; tmp[k] = d < 10 ? '0'+d : 'a'+d-10; v >>= 4; }
            tmp[16] = 0;
            for (p = tmp; *p; p++) rep[n++] = *p;
        }
        for (p = " pid="; *p; p++) rep[n++] = *p;
        {
            char tmp[20]; int t = 0; long v = pid;
            if (v == 0) tmp[t++] = '0';
            while (v) { tmp[t++] = '0' + (v % 10); v /= 10; }
            while (t) rep[n++] = tmp[--t];
        }
        for (p = " k0="; *p; p++) rep[n++] = *p;
        {
            char tmp[24]; int t = 0; long v = kill_rc0;
            if (v == 0) tmp[t++] = '0';
            while (v) { int d = v % 10; if (d < 0) d = -d; tmp[t++] = '0' + d; v /= 10; }
            if (kill_rc0 < 0) tmp[t++] = '-';
            while (t) rep[n++] = tmp[--t];
        }
        for (p = (g_count == (uint64_t)rounds) ? " PASS\n" : " FAIL\n"; *p; p++) rep[n++] = *p;
        sys(__NR_write, 1, (long)rep, n, 0);
        sys(__NR_exit, g_count == (uint64_t)rounds ? 0 : 42, 0, 0, 0);
    }
}
