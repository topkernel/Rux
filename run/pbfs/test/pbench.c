/*
 * pbench.c — P1-P7 microbench probe (perf-report.md section 8.2).
 *
 * One static musl x86_64 binary that runs IDENTICALLY on the Rux
 * main-class kernel and on the OHOS 6.6.101 reference kernel, so every
 * number is a same-binary pair. Runs as init (init=/test/pbench) for
 * quiet probe boots, or as a late service on the OH image — behavior
 * only depends on argv.
 *
 * Timing: per-op rdtsc deltas, calibrated per-run against
 * CLOCK_MONOTONIC_RAW (nr 4; supported on Rux, kernel time.rs). Medians
 * and p99 over fixed N with warmup discarded — means stay useless under
 * foreign-QEMU host noise. Each bench also carries wall=<us/op> from a
 * MONOTONIC_RAW pair around the whole measured loop, so if rdtsc ever
 * misbehaves on one side the wall number still stands alone.
 *
 * Output — one parsable line per bench, to stdout AND to a results file:
 *   PBENCH <name> n=<N> med=<us> p99=<us> min=<us> max=<us> avg=<us> wall=<us> [extra]
 *   PBENCH CAL tsc_hz=<n>        (calibration, first)
 *   PBENCH PREP ... / PBENCH <name> SKIP why=<reason>
 *   PBENCH DONE rc=0             (last, before poweroff)
 * A bench that hits an unsupported primitive SKIPs itself; the rest of
 * the run continues.
 *
 * Benches (perf-report.md 8.2):
 *   P1 p1_null / p1_clock        syscall(SYS_getuid) / clock_gettime loops, N=1e6
 *   P2 p2_ctxsw                  1-byte pipe ping-pong, parent pinned CPU0,
 *                                child CPU1, N=1e5, values are HALF roundtrip
 *   P3 p3a_fault_anon            first-touch anon, 64 MB map, per-page
 *      p3b_fault_cow             child rewrites parent-touched pages (COW)
 *      p3c_fault_file            mmap+touch of a file the probe wrote (16 MB)
 *   P4 p4_fork_exit / p4_fork_exec  N=1e3; exec target is THIS binary
 *                                re-exec'd as "<self> -x" (immediate _exit(0))
 *                                — identical size on both sides, no second
 *                                file to ship
 *   P5 p5a_pipe                  1-byte pipe roundtrip, no pinning, N=1e5
 *      p5b_dgram_send            unix dgram socketpair, 74 B sendto, N=2e4
 *      p5b_dgram_recv_peek       receiver side: recvfrom(MSG_PEEK)+recvfrom
 *                                per record = the hilogd protocol shape
 *      p5c_stream_write          unix stream socketpair, 74 B write, N=1e5
 *      p5e/p5f (leg 46)          flow-control discriminators: p5c2 shape
 *                                with an O_NONBLOCK spin-read child (p5e)
 *                                / parent MSG_DONTWAIT+EAGAIN spin (p5f)
 *   P6 p6_stat / p6_openclose / p6_getdents / p6_read_cached /
 *      p6_write_fsync / p6_append / p6_mkdrmdir / p6_unlink
 *                                on a prep-built tree (1000 files x 4 KB)
 *                                under <workdir>/pbench-tree
 *   P7 p7_sleep1ms               nanosleep(1 ms) x 1e4, med/p99/max = OVERSHOOT
 *
 * Usage: pbench [workdir] [results-file]
 *   default workdir=/test, results=<workdir>/pbench-results.txt
 *   workdir must be on the ext4 under test (rootfs for quiet boots,
 *   /data for the OH run-day).
 *
 * Build: toolchain/x86_64-rux-linux-musl/bin/musl-gcc -static -O2
 * End: reboot(POWER_OFF) — as init the kernel syncs and powers down; if
 * reboot fails, plain exit (init exit is survivable, boot script keys on
 * the DONE marker).
 */
#define _GNU_SOURCE             /* cpu_set_t / CPU_SET in musl sched.h */
#include <errno.h>
#include <fcntl.h>
#include <sched.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

typedef unsigned long long u64;
typedef long long s64;
typedef unsigned int u32;

/* ---------------- tunables (fixed N per design) ---------------- */
#define P1_N       1000000
#define P2_N       100000
#define P3A_PAGES  16384        /* 64 MB */
#define P3B_PAGES  8192         /* 32 MB shared before fork */
#define P3C_BYTES  (16UL * 1024 * 1024)
#define P4_N       1000
#define P5A_N      100000
#define P5B_N      20000
#define P5C_N      100000
#define P6_FILES   1000
#define P6_ROUNDS  10           /* x P6_FILES = 1e4 samples for stat/oc/rc */
#define P6_SYNC_N  1000
#define P7_N       10000

#define REC_BYTES  74           /* hilogd record shape for P5b/P5c */

static double g_tsc_hz;         /* 0 = rdtsc unusable, med/p99 print as -1 */
static int g_out_fd = -1;       /* results file, -1 = stdout only */
static char g_self[256] = "/test/pbench";
static char g_work[256] = "/test";

/* ---------------- tiny formatted output ---------------- */
static void emit_len(const char *s, size_t n)
{
    ssize_t w = write(1, s, n);
    (void)w;
    if (g_out_fd >= 0) {
        w = write(g_out_fd, s, n);
        (void)w;
    }
}
static void emit(const char *s) { emit_len(s, strlen(s)); }

/* us with 3 decimals from ns, into buf (>=26B) */
static void fmt_us(char *buf, double ns)
{
    if (ns < 0) { strcpy(buf, "-1"); return; }
    u64 v = (u64)(ns + 0.5);
    u64 whole = v / 1000, frac = v % 1000;
    char fr[4] = { (char)('0' + frac / 100), (char)('0' + (frac / 10) % 10), (char)('0' + frac % 10), 0 };
    char wb[26];
    int i = (int)sizeof(wb) - 1;
    wb[i] = 0;                       /* keep the last slot for the NUL */
    if (!whole) wb[--i] = '0';
    while (whole && i > 0) { wb[--i] = (char)('0' + whole % 10); whole /= 10; }
    strcpy(buf, wb + i); strcat(buf, "."); strcat(buf, fr);
}

/* ---------------- clocks ---------------- */
static inline u64 rdtsc_(void)
{
    unsigned lo, hi;
    __asm__ __volatile__("rdtsc" : "=a"(lo), "=d"(hi));
    return ((u64)hi << 32) | lo;
}

static s64 mnr_ns(void)         /* CLOCK_MONOTONIC_RAW */
{
    struct timespec ts;
    syscall(SYS_clock_gettime, 4, &ts);
    return (s64)ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

static int cmp_u64(const void *a, const void *b)
{
    u64 x = *(const u64 *)a, y = *(const u64 *)b;
    return x < y ? -1 : (x > y ? 1 : 0);
}

struct pstats { u64 med, p99, min, max, sum; };

static void stats_of(u64 *t, size_t n, struct pstats *s)
{
    if (!n) { memset(s, 0, sizeof(*s)); return; }
    qsort(t, n, sizeof(u64), cmp_u64);
    s->min = t[0];
    s->med = t[n / 2];
    s->p99 = t[(n * 99) / 100 >= n ? n - 1 : (n * 99) / 100];
    s->max = t[n - 1];
    u64 acc = 0;
    for (size_t i = 0; i < n; i++) acc += t[i];
    s->sum = acc;
}

/* print one PBENCH line from precomputed stats (ticks) + wall ns */
static void report_s(const char *name, size_t n, const struct pstats *s,
                     s64 wall_ns, const char *extra)
{
    char b[512], m[24], p9[24], mn[24], mx[24], av[24], wl[24];
    double k;                       /* ns per tick */
    if (g_tsc_hz > 0) k = 1e9 / g_tsc_hz; else k = -1;
    if (g_tsc_hz > 0 && n) {
        fmt_us(m, (double)s->med * k);
        fmt_us(p9, (double)s->p99 * k);
        fmt_us(mn, (double)s->min * k);
        fmt_us(mx, (double)s->max * k);
        fmt_us(av, (double)s->sum * k / (double)n);
    } else {
        strcpy(m, "-1"); strcpy(p9, "-1"); strcpy(mn, "-1");
        strcpy(mx, "-1"); strcpy(av, "-1");
    }
    fmt_us(wl, n ? (double)wall_ns / (double)n : (double)wall_ns);
    int len = snprintf(b, sizeof(b),
                       "PBENCH %s n=%zu med=%s p99=%s min=%s max=%s avg=%s wall=%s%s%s\n",
                       name, n, m, p9, mn, mx, av, wl,
                       extra && *extra ? " " : "", extra ? extra : "");
    if (len > 0 && (size_t)len < sizeof(b)) emit_len(b, (size_t)len);
}

static void report_ticks(const char *name, u64 *t, size_t n, s64 wall_ns,
                         const char *extra)
{
    struct pstats s;
    stats_of(t, n, &s);
    report_s(name, n, &s, wall_ns, extra);
}

static void skip_line(const char *name, const char *why)
{
    char b[256];
    int len = snprintf(b, sizeof(b), "PBENCH %s SKIP why=%s\n", name, why);
    if (len > 0) emit_len(b, (size_t)len);
}

static u64 *xmalloc_samples(size_t n)
{
    u64 *p = malloc(n * sizeof(u64));
    if (!p) skip_line("alloc", "oom");
    return p;
}

/* ---------------- calibration ---------------- */
static void calibrate(void)
{
    /* 3 rounds of ~50 ms; median hz; reject implausible values */
    u64 hz[3];
    int ok = 0;
    for (int r = 0; r < 3; r++) {
        u64 t0 = rdtsc_();
        s64 c0 = mnr_ns();
        while (mnr_ns() - c0 < 50000000LL) ; /* ~50 ms */
        s64 c1 = mnr_ns();
        u64 t1 = rdtsc_();
        s64 dns = c1 - c0;
        if (dns > 1000000LL && t1 > t0) {
            hz[ok++] = (u64)((double)(t1 - t0) * 1e9 / (double)dns);
        }
    }
    if (ok == 3) {
        qsort(hz, 3, sizeof(u64), cmp_u64);
        if (hz[1] > 1000000ULL && hz[1] < 20000000000000ULL)
            g_tsc_hz = (double)hz[1];
    }
    char b[128];
    int len = snprintf(b, sizeof(b), "PBENCH CAL tsc_hz=%llu\n",
                       g_tsc_hz > 0 ? (unsigned long long)g_tsc_hz : 0ULL);
    if (len > 0) emit_len(b, (size_t)len);
}

/* ---------------- P1: null syscall + clock_gettime ---------------- */
static void b_p1(void)
{
    u64 *t = xmalloc_samples(P1_N);
    s64 w;
    if (t) {
        for (int i = 0; i < 20000; i++) syscall(SYS_getuid);
        w = mnr_ns();
        for (int i = 0; i < P1_N; i++) {
            u64 t0 = rdtsc_();
            syscall(SYS_getuid);
            u64 t1 = rdtsc_();
            t[i] = t1 - t0;
        }
        w = mnr_ns() - w;
        report_ticks("p1_null", t, P1_N, w, "");
        free(t);
    }

    t = xmalloc_samples(P1_N);
    if (t) {
        struct timespec ts;
        for (int i = 0; i < 20000; i++) syscall(SYS_clock_gettime, 1, &ts);
        w = mnr_ns();
        for (int i = 0; i < P1_N; i++) {
            u64 t0 = rdtsc_();
            syscall(SYS_clock_gettime, 1, &ts);
            u64 t1 = rdtsc_();
            t[i] = t1 - t0;
        }
        w = mnr_ns() - w;
        report_ticks("p1_clock", t, P1_N, w, "");
        free(t);
    }
}

/* ---------------- shared ping-pong child ---------------- */
/* reads 1 byte from rfd, writes 1 byte to wfd, until error/EOF;
 * pins itself to cpu if cpu >= 0. Pipe roundtrips need BOTH ends here
 * (a pipe's read end cannot be written to). */
static void ping_pong_child(int rfd, int wfd, int cpu)
{
    if (cpu >= 0) {
        cpu_set_t set;
        CPU_ZERO(&set);
        CPU_SET(cpu, &set);
        sched_setaffinity(0, sizeof(set), &set);
    }
    char c;
    for (;;) {
        ssize_t r = read(rfd, &c, 1);
        if (r != 1) break;
        if (write(wfd, &c, 1) != 1) break;
    }
    _exit(0);
}

/* ---------------- P2: pinned context switch ---------------- */
static int g_aff_ok;

static void b_p2(void)
{
    int pfd[2];
    if (pipe(pfd) < 0) { skip_line("p2_ctxsw", "pipe-errno"); return; }
    pid_t pid = fork();
    if (pid < 0) { skip_line("p2_ctxsw", "fork-errno"); close(pfd[0]); close(pfd[1]); return; }
    if (pid == 0) ping_pong_child(pfd[0], pfd[1], 1);

    g_aff_ok = 0;
    cpu_set_t set;
    CPU_ZERO(&set);
    CPU_SET(0, &set);
    if (sched_setaffinity(0, sizeof(set), &set) == 0) g_aff_ok = 1;

    u64 *t = xmalloc_samples(P2_N);
    if (!t) { close(pfd[0]); close(pfd[1]); waitpid(pid, 0, 0); return; }

    char c = 1;
    /* warmup */
    for (int i = 0; i < 1000; i++) {
        if (write(pfd[1], &c, 1) != 1 || read(pfd[0], &c, 1) != 1) break;
    }
    size_t n = 0;
    s64 w = mnr_ns();
    for (int i = 0; i < P2_N; i++) {
        u64 t0 = rdtsc_();
        if (write(pfd[1], &c, 1) != 1) break;
        if (read(pfd[0], &c, 1) != 1) break;
        u64 t1 = rdtsc_();
        t[n++] = (t1 - t0) / 2;      /* half roundtrip */
    }
    w = mnr_ns() - w;
    close(pfd[0]); close(pfd[1]);
    /* the echo child holds the write end open, so it never sees EOF:
     * stop it explicitly, then reap */
    kill(pid, SIGKILL);
    waitpid(pid, 0, 0);
    char extra[32];
    snprintf(extra, sizeof(extra), "aff=%d%s", g_aff_ok, n < (size_t)P2_N ? " child_died=1" : "");
    report_ticks("p2_ctxsw", t, n, w / 2, extra);
    free(t);
}

/* ---------------- P3: fault classes ---------------- */
static void b_p3a(void)
{
    /* warmup on a small separate map */
    char *warm = mmap(0, 2UL * 1024 * 1024, PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (warm == MAP_FAILED) { skip_line("p3a_fault_anon", "mmap-warmup"); return; }
    for (size_t i = 0; i < 2UL * 1024 * 1024 / 4096; i++) *(volatile char *)(warm + i * 4096) = 1;
    munmap(warm, 2UL * 1024 * 1024);

    size_t bytes = (size_t)P3A_PAGES * 4096;
    char *p = mmap(0, bytes, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) { skip_line("p3a_fault_anon", "mmap-errno"); return; }
    u64 *t = xmalloc_samples(P3A_PAGES);
    if (!t) { munmap(p, bytes); return; }
    s64 wt = mnr_ns();
    size_t n = 0;
    for (size_t i = 0; i < P3A_PAGES; i++) {
        u64 t0 = rdtsc_();
        *(volatile char *)(p + i * 4096) = (char)i;
        u64 t1 = rdtsc_();
        t[n++] = t1 - t0;
    }
    s64 w = mnr_ns() - wt;
    report_ticks("p3a_fault_anon", t, n, w, "");
    free(t);
    munmap(p, bytes);
}

/* child summary handed to parent over a pipe (raw ticks + wall) */
struct child_sum { size_t n; struct pstats s; s64 wall_ns; u64 errno_hit; };

static void b_p3b(void)
{
    size_t bytes = (size_t)P3B_PAGES * 4096;
    char *p = mmap(0, bytes, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) { skip_line("p3b_fault_cow", "mmap-errno"); return; }
    for (size_t i = 0; i < P3B_PAGES; i++) *(volatile char *)(p + i * 4096) = 1;

    int sum_pipe[2];
    if (pipe(sum_pipe) < 0) { skip_line("p3b_fault_cow", "pipe-errno"); munmap(p, bytes); return; }

    pid_t pid = fork();
    if (pid < 0) { skip_line("p3b_fault_cow", "fork-errno"); munmap(p, bytes); return; }
    if (pid == 0) {
        /* child: rewrite every page -> COW fault each */
        close(sum_pipe[0]);
        u64 *t = malloc(P3B_PAGES * sizeof(u64));
        if (!t) _exit(42);
        s64 wt = mnr_ns();
        for (size_t i = 0; i < P3B_PAGES; i++) {
            u64 t0 = rdtsc_();
            *(volatile char *)(p + i * 4096) ^= 2;
            u64 t1 = rdtsc_();
            t[i] = t1 - t0;
        }
        s64 w = mnr_ns() - wt;
        struct child_sum cs;
        cs.n = P3B_PAGES;
        stats_of(t, P3B_PAGES, &cs.s);
        cs.wall_ns = w;
        cs.errno_hit = 0;
        if (write(sum_pipe[1], &cs, sizeof(cs)) != (ssize_t)sizeof(cs)) _exit(43);
        _exit(0);
    }
    close(sum_pipe[1]);
    struct child_sum cs;
    ssize_t r = read(sum_pipe[0], &cs, sizeof(cs));
    close(sum_pipe[0]);
    int st; waitpid(pid, &st, 0);
    if (r == (ssize_t)sizeof(cs)) {
        report_s("p3b_fault_cow", cs.n, &cs.s, cs.wall_ns, "measured_by=child");
    } else {
        skip_line("p3b_fault_cow", "child-summary-lost");
    }
    munmap(p, bytes);
}

static void b_p3c(const char *path)
{
    /* prep: write the 16 MB file (counted under PREP) */
    int fd = open(path, O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) { skip_line("p3c_fault_file", "open-create-errno"); return; }
    char blk[65536];
    u32 seed = 12345;
    for (size_t i = 0; i < sizeof(blk); i++) { seed = seed * 1103515245 + 12345; blk[i] = (char)(seed >> 16); }
    for (size_t off = 0; off < P3C_BYTES; off += sizeof(blk)) {
        if (write(fd, blk, sizeof(blk)) != (ssize_t)sizeof(blk)) {
            skip_line("p3c_fault_file", "write-errno"); close(fd); return;
        }
    }
    fsync(fd);
    close(fd);

    /* bench: mmap read-only, touch every page */
    fd = open(path, O_RDONLY);
    if (fd < 0) { skip_line("p3c_fault_file", "open-ro-errno"); return; }
    size_t pages = P3C_BYTES / 4096;
    char *p = mmap(0, P3C_BYTES, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED) { skip_line("p3c_fault_file", "mmap-errno"); close(fd); return; }
    u64 *t = xmalloc_samples(pages);
    if (!t) { munmap(p, P3C_BYTES); close(fd); return; }
    s64 wt = mnr_ns();
    size_t n = 0;
    for (size_t i = 0; i < pages; i++) {
        u64 t0 = rdtsc_();
        char c = *(volatile char *)(p + i * 4096);
        u64 t1 = rdtsc_();
        t[n++] = t1 - t0;
        (void)c;
    }
    s64 w = mnr_ns() - wt;
    report_ticks("p3c_fault_file", t, n, w, "");
    free(t);
    munmap(p, P3C_BYTES);
    close(fd);
}

/* ---------------- P3d: CONCURRENT file-fault contention ----------------
 * Leg-56B instrument debt (ledger 8.10.27, coordinator-acknowledged):
 * the P3 rows are single-process warm-cache and cannot see the global
 * inodes-lock contention the OH load phase creates. 4 processes, each
 * mmap-touching a DISTINCT file's pages; per-proc fault medians +
 * a combined wall row. Gated off by default (env P3D=1).
 */
static void b_p3d(const char *dir)
{
    const char *ev = getenv("P3D");
    if (!ev || !ev[0]) return;                 /* gated off by default */

    const int NPROC = 4;
    const size_t per_file_bytes = 4u << 20;    /* 4 MB = 1024 pages/proc */
    char paths[NPROC][256];
    int ok = 1;
    for (int i = 0; i < NPROC; i++) {
        snprintf(paths[i], sizeof(paths[i]), "%s/p3d-file-%d", dir, i);
        int fd = open(paths[i], O_CREAT | O_RDWR | O_TRUNC, 0644);
        if (fd < 0) { ok = 0; break; }
        static char blk[4096];
        for (size_t off = 0; off < per_file_bytes; off += sizeof(blk)) {
            if (write(fd, blk, sizeof(blk)) != (ssize_t)sizeof(blk)) { ok = 0; break; }
        }
        fsync(fd);
        close(fd);
        if (!ok) break;
    }
    if (!ok) { skip_line("p3d_fault_conc", "prep-errno"); return; }

    size_t pages = per_file_bytes / 4096;
    int pipefd[NPROC][2];
    for (int i = 0; i < NPROC; i++) if (pipe(pipefd[i]) < 0) ok = 0;
    if (!ok) { skip_line("p3d_fault_conc", "pipe-errno"); return; }

    s64 wall0 = mnr_ns();
    for (int i = 0; i < NPROC; i++) {
        pid_t pid = fork();
        if (pid == 0) {
            /* child i: mmap its OWN file, touch every page */
            close(pipefd[i][0]);
            int fd = open(paths[i], O_RDONLY);
            if (fd < 0) _exit(1);
            char *p = mmap(0, per_file_bytes, PROT_READ, MAP_PRIVATE, fd, 0);
            if (p == MAP_FAILED) _exit(2);
            u64 *t = xmalloc_samples(pages);
            if (!t) _exit(3);
            size_t n = 0;
            u64 acc = 0;
            for (size_t pg = 0; pg < pages; pg++) {
                u64 t0 = rdtsc_();
                char c = *(volatile char *)(p + pg * 4096);
                u64 t1 = rdtsc_();
                t[n++] = t1 - t0;
                acc += t1 - t0;
                (void)c;
            }
            /* summary through the pipe, p3b's shape */
            struct child_sum cs;
            cs.n = n;
            stats_of(t, n, &cs.s);
            cs.wall_ns = 0;
            cs.errno_hit = 0;
            (void)acc;
            if (write(pipefd[i][1], &cs, sizeof(cs)) != (ssize_t)sizeof(cs)) _exit(44);
            _exit(0);
        }
        if (pid < 0) { ok = 0; break; }
        close(pipefd[i][1]);
    }
    if (!ok) { skip_line("p3d_fault_conc", "fork-errno"); return; }
    for (int i = 0; i < NPROC; i++) {
        struct child_sum cs;
        ssize_t r = read(pipefd[i][0], &cs, sizeof(cs));
        close(pipefd[i][0]);
        int st; waitpid(-1, &st, 0);
        if (r == (ssize_t)sizeof(cs)) {
            report_s("p3d_fault_conc", cs.n, &cs.s, 0, "measured_by=child_proc");
        } else {
            skip_line("p3d_fault_conc", "child-summary-lost");
        }
    }
    s64 wall = mnr_ns() - wall0;
    {
        char note[64];
        snprintf(note, sizeof(note), "ns_per_fault_4proc_wall=%lld", (long long)(wall / (s64)(NPROC * (long long)pages)));
        emit_len(note, strlen(note));
    }
    for (int i = 0; i < NPROC; i++) unlink(paths[i]);
}

/* ---------------- P4: fork+exit / fork+exec ---------------- */
static void b_p4(void)
{
    u64 *t = xmalloc_samples(P4_N);
    if (t) {
        for (int i = 0; i < 20; i++) {
            pid_t pid = fork();
            if (pid == 0) _exit(0);
            if (pid > 0) waitpid(pid, 0, 0);
        }
        s64 w = mnr_ns();
        size_t n = 0;
        for (int i = 0; i < P4_N; i++) {
            u64 t0 = rdtsc_();
            pid_t pid = fork();
            if (pid == 0) _exit(0);
            if (pid < 0) break;
            waitpid(pid, 0, 0);
            u64 t1 = rdtsc_();
            t[n++] = t1 - t0;
        }
        w = mnr_ns() - w;
        report_ticks("p4_fork_exit", t, n, w, "");
        free(t);
    }

    t = xmalloc_samples(P4_N);
    if (!t) return;
    char *av_x[3]; av_x[0] = g_self; av_x[1] = (char *)"-x"; av_x[2] = 0;
    char *ev[1]; ev[0] = 0;
    /* warmup */
    for (int i = 0; i < 20; i++) {
        pid_t pid = fork();
        if (pid == 0) { execve(g_self, av_x, ev); _exit(111); }
        if (pid > 0) waitpid(pid, 0, 0);
    }
    s64 w = mnr_ns();
    size_t n = 0;
    for (int i = 0; i < P4_N; i++) {
        u64 t0 = rdtsc_();
        pid_t pid = fork();
        if (pid == 0) { execve(g_self, av_x, ev); _exit(111); }
        if (pid < 0) break;
        int st; waitpid(pid, &st, 0);
        u64 t1 = rdtsc_();
        if (!(WIFEXITED(st) && WEXITSTATUS(st) == 0)) break;
        t[n++] = t1 - t0;
    }
    w = mnr_ns() - w;
    report_ticks("p4_fork_exec", t, n, w,
                 n < (size_t)P4_N ? "exec_bad=1" : "exec_target=self");
    free(t);
}

/* ---------------- P5: pipe + unix socket roundtrips ---------------- */
static void b_p5a(void)
{
    int pfd[2];
    if (pipe(pfd) < 0) { skip_line("p5a_pipe", "pipe-errno"); return; }
    pid_t pid = fork();
    if (pid < 0) { skip_line("p5a_pipe", "fork-errno"); close(pfd[0]); close(pfd[1]); return; }
    if (pid == 0) ping_pong_child(pfd[0], pfd[1], -1);
    u64 *t = xmalloc_samples(P5A_N);
    if (!t) { close(pfd[0]); close(pfd[1]); waitpid(pid, 0, 0); return; }
    char c = 7;
    for (int i = 0; i < 1000; i++) {
        if (write(pfd[1], &c, 1) != 1 || read(pfd[0], &c, 1) != 1) break;
    }
    size_t n = 0;
    s64 w = mnr_ns();
    for (int i = 0; i < P5A_N; i++) {
        u64 t0 = rdtsc_();
        if (write(pfd[1], &c, 1) != 1) break;
        if (read(pfd[0], &c, 1) != 1) break;
        u64 t1 = rdtsc_();
        t[n++] = t1 - t0;          /* full roundtrip, no pinning */
    }
    w = mnr_ns() - w;
    close(pfd[0]); close(pfd[1]);
    kill(pid, SIGKILL);          /* echo child never sees EOF (see P2) */
    waitpid(pid, 0, 0);
    report_ticks("p5a_pipe", t, n, w, n < (size_t)P5A_N ? "child_died=1" : "");
    free(t);
}

static void b_p5b(void)
{
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_DGRAM, 0, sv) < 0) {
        skip_line("p5b_dgram_send", "socketpair-dgram-errno");
        skip_line("p5b_dgram_recv_peek", "socketpair-dgram-errno");
        return;
    }
    int ctl[2];                    /* child -> parent summary */
    if (pipe(ctl) < 0) {
        skip_line("p5b_dgram_send", "pipe-errno");
        skip_line("p5b_dgram_recv_peek", "pipe-errno");
        close(sv[0]); close(sv[1]);
        return;
    }
    pid_t pid = fork();
    if (pid < 0) {
        skip_line("p5b_dgram_send", "fork-errno");
        skip_line("p5b_dgram_recv_peek", "fork-errno");
        close(sv[0]); close(sv[1]); close(ctl[0]); close(ctl[1]);
        return;
    }
    if (pid == 0) {
        /* receiver: PEEK then consume per record; times the pair */
        close(sv[0]); close(ctl[0]);
        u64 *t = malloc(P5B_N * sizeof(u64));
        if (!t) _exit(42);
        char buf[512];
        size_t n = 0;
        for (;;) {
            u64 t0 = rdtsc_();
            ssize_t r = recvfrom(sv[1], buf, sizeof(buf), MSG_PEEK, 0, 0);
            if (r < 0) break;
            if (r == 1 && buf[0] == 'E') { recvfrom(sv[1], buf, sizeof(buf), 0, 0, 0); break; }
            ssize_t r2 = recvfrom(sv[1], buf, sizeof(buf), 0, 0, 0);
            u64 t1 = rdtsc_();
            if (r2 < 0) break;
            if (n < P5B_N) t[n++] = t1 - t0;
            if (r2 < r) break;
        }
        struct child_sum cs;
        cs.n = n;
        stats_of(t, n, &cs.s);
        cs.wall_ns = 0;
        cs.errno_hit = 0;
        if (write(ctl[1], &cs, sizeof(cs)) != (ssize_t)sizeof(cs)) _exit(43);
        _exit(0);
    }
    close(sv[1]); close(ctl[1]);
    u64 *t = xmalloc_samples(P5B_N);
    if (!t) { close(sv[0]); close(ctl[0]); waitpid(pid, 0, 0); return; }
    char rec[REC_BYTES];
    memset(rec, 0x5a, sizeof(rec));
    for (int i = 0; i < 1000; i++) sendto(sv[0], rec, REC_BYTES, 0, 0, 0);
    size_t n = 0;
    s64 w = mnr_ns();
    for (int i = 0; i < P5B_N; i++) {
        u64 t0 = rdtsc_();
        ssize_t r = sendto(sv[0], rec, REC_BYTES, 0, 0, 0);
        u64 t1 = rdtsc_();
        if (r != REC_BYTES) break;
        t[n++] = t1 - t0;
    }
    w = mnr_ns() - w;
    /* end marker + collect child summary */
    char end = 'E';
    sendto(sv[0], &end, 1, 0, 0, 0);
    struct child_sum cs;
    ssize_t rr = read(ctl[0], &cs, sizeof(cs));
    int st; waitpid(pid, &st, 0);
    report_ticks("p5b_dgram_send", t, n, w, n < (size_t)P5B_N ? "send_short=1" : "");
    free(t);
    if (rr == (ssize_t)sizeof(cs) && cs.n)
        report_s("p5b_dgram_recv_peek", cs.n, &cs.s, 0, "measured_by=child peek+consume");
    else
        skip_line("p5b_dgram_recv_peek", "child-summary-lost");
    close(sv[0]); close(ctl[0]);
}

static void b_p5c(void)
{
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        skip_line("p5c_stream_write", "socketpair-stream-errno");
        return;
    }
    pid_t pid = fork();
    if (pid < 0) { skip_line("p5c_stream_write", "fork-errno"); close(sv[0]); close(sv[1]); return; }
    if (pid == 0) {
        close(sv[0]);
        char buf[65536];
        while (read(sv[1], buf, sizeof(buf)) > 0) ;
        _exit(0);
    }
    close(sv[1]);
    u64 *t = xmalloc_samples(P5C_N);
    if (!t) { close(sv[0]); waitpid(pid, 0, 0); return; }
    char rec[REC_BYTES];
    memset(rec, 0xa3, sizeof(rec));
    for (int i = 0; i < 2000; i++) { if (write(sv[0], rec, REC_BYTES) != REC_BYTES) break; }
    size_t n = 0;
    s64 w = mnr_ns();
    for (int i = 0; i < P5C_N; i++) {
        u64 t0 = rdtsc_();
        ssize_t r = write(sv[0], rec, REC_BYTES);
        u64 t1 = rdtsc_();
        if (r != REC_BYTES) break;
        t[n++] = t1 - t0;
    }
    w = mnr_ns() - w;
    close(sv[0]);
    waitpid(pid, 0, 0);
    report_ticks("p5c_stream_write", t, n, w, n < (size_t)P5C_N ? "write_short=1" : "");
    free(t);
}

/* p5c2/p5c3 (leg 43): entry-path decomposition of the p5c row. p5c
 * times write(2) on a stream socketpair with a read(2)-draining
 * child and reads 41-44 us on EVERY boot/kernel while the dgram
 * p5b sendto row reads 4.9-5.5 fast-regime. Same transport, same
 * locks, same wake shape — the delta must live in the WRAPPERS:
 * write(2)/read(2) on a socket fd go through the chunked staging
 * path (per-call staging alloc + double copy + a per-call
 * File::path() format for the pathstash hook) while sendto/recvfrom
 * enter the socket ops directly. p5c2 = sendto/recvfrom on the SAME
 * stream socketpair; p5c3 = write(2) parent, recvfrom(2)-draining
 * child (splits the write-side vs read-side wrapper cost).
 * Reading: p5c2 ~p5b-fast  => the wrappers own the 38 us (fix the
 *           sys_write/sys_read socket path); p5c2 ~p5c  => the stream
 *           transport/wake shape owns it. p5c3 splits further. */
static void b_p5c23(int use_send, int child_recv)
{
    const char *tag = use_send ? (child_recv ? "p5c3_stream_wrecv" : "p5c2_stream_send")
                               : "p5c_stream_write";
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        skip_line(tag, "socketpair-stream-errno");
        return;
    }
    pid_t pid = fork();
    if (pid < 0) { skip_line(tag, "fork-errno"); close(sv[0]); close(sv[1]); return; }
    if (pid == 0) {
        close(sv[0]);
        char buf[65536];
        if (child_recv) { while (recvfrom(sv[1], buf, sizeof(buf), 0, 0, 0) > 0) ; }
        else            { while (read(sv[1], buf, sizeof(buf)) > 0) ; }
        _exit(0);
    }
    close(sv[1]);
    u64 *t = xmalloc_samples(P5C_N);
    if (!t) { close(sv[0]); waitpid(pid, 0, 0); return; }
    char rec[REC_BYTES];
    memset(rec, 0xa3, sizeof(rec));
    for (int i = 0; i < 2000; i++) {
        ssize_t r = use_send ? sendto(sv[0], rec, REC_BYTES, 0, 0, 0)
                             : write(sv[0], rec, REC_BYTES);
        if (r != REC_BYTES) break;
    }
    size_t n = 0;
    s64 w = mnr_ns();
    for (int i = 0; i < P5C_N; i++) {
        u64 t0 = rdtsc_();
        ssize_t r = use_send ? sendto(sv[0], rec, REC_BYTES, 0, 0, 0)
                             : write(sv[0], rec, REC_BYTES);
        u64 t1 = rdtsc_();
        if (r != REC_BYTES) break;
        t[n++] = t1 - t0;
    }
    w = mnr_ns() - w;
    close(sv[0]);
    waitpid(pid, 0, 0);
    report_ticks(tag, t, n, w, n < (size_t)P5C_N ? "short=1" : "");
    free(t);
}

static void b_p5c2(void) { b_p5c23(1, 0); }
static void b_p5c3(void) { b_p5c23(1, 1); }

/* p5d (leg 44): in-process socketpair round trip, no second task.
 * sendto then drain on sv[1] with EITHER read(2) (p5d_rt_read) or
 * recvfrom(2) (p5d_rt_recv) in the SAME loop iteration. This isolates
 * the read(2)-on-socket kernel path from the child-process drain
 * dynamics the p5c/p5c2/p5c3 rows couple through flow control.
 * Reading: p5d_rt_read ~p5d_rt_recv => the read(2) wrapper is fixed
 *           (the p5c gap lives in the cross-process coupling);
 *           p5d_rt_read - p5d_rt_recv >> 0 => the wrapper is still
 *           slow (dispatch bug). */
static void b_p5d_rt(int use_recv)
{
    const char *tag = use_recv ? "p5d_rt_recv" : "p5d_rt_read";
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        skip_line(tag, "socketpair-stream-errno");
        return;
    }
    u64 *t = xmalloc_samples(P5C_N);
    if (!t) { close(sv[0]); close(sv[1]); return; }
    char rec[REC_BYTES];
    char buf[REC_BYTES * 2];
    memset(rec, 0xa5, sizeof(rec));
    for (int i = 0; i < 2000; i++) {
        if (sendto(sv[0], rec, REC_BYTES, 0, 0, 0) != REC_BYTES) break;
        ssize_t r = use_recv ? recvfrom(sv[1], buf, sizeof(buf), 0, 0, 0)
                             : read(sv[1], buf, sizeof(buf));
        if (r != REC_BYTES) break;
    }
    size_t n = 0;
    s64 w = mnr_ns();
    for (int i = 0; i < P5C_N; i++) {
        u64 t0 = rdtsc_();
        ssize_t s = sendto(sv[0], rec, REC_BYTES, 0, 0, 0);
        ssize_t r = s == REC_BYTES
            ? (use_recv ? recvfrom(sv[1], buf, sizeof(buf), 0, 0, 0)
                        : read(sv[1], buf, sizeof(buf)))
            : -1;
        u64 t1 = rdtsc_();
        if (s != REC_BYTES || r != REC_BYTES) break;
        t[n++] = t1 - t0;
    }
    w = mnr_ns() - w;
    close(sv[0]); close(sv[1]);
    report_ticks(tag, t, n, w, n < (size_t)P5C_N ? "short=1" : "");
    free(t);
}

static void b_p5d_read(void) { b_p5d_rt(0); }
static void b_p5d_recv(void) { b_p5d_rt(1); }

/* p5e (leg 46): p5c2's exact shape (parent sendto on a stream socketpair,
 * child drains with read(2)) but the child end is O_NONBLOCK and the
 * child spins — it NEVER sleeps. Discriminates the child's sleep/wake
 * handshake from queue-depth/per-call drain cost for the 40-44 us parent
 * class (ledger 8.10.14's armed discriminator):
 *   p5e ~p5c3-fast  => the sleeping child is required for the class; the
 *                      mechanism is the cross-process wake dance (child
 *                      sleeps -> queue fills to the enqueue gate -> parent
 *                      blocks on room -> child wake->drain -> parent
 *                      wake->pick; the recv-child's faster drain cycle
 *                      keeps the queue under the cap and the parent never
 *                      pays the block).
 *   p5e ~p5c2-slow  => even a spinning consumer leaves the parent slow;
 *                      the enqueue path at queue depth or the per-call
 *                      read() drain cost (staging alloc) owns the class. */
static void b_p5e(void)
{
    int sv[2], ctl[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0 || pipe(ctl) < 0) {
        skip_line("p5e_stream_send", "socketpair-pipe-errno");
        close(sv[0]); close(sv[1]);
        return;
    }
    pid_t pid = fork();
    if (pid < 0) {
        skip_line("p5e_stream_send", "fork-errno");
        skip_line("p5e_child_spin", "fork-errno");
        close(sv[0]); close(sv[1]); close(ctl[0]); close(ctl[1]);
        return;
    }
    if (pid == 0) {
        /* spinning drain: O_NONBLOCK read, EAGAIN retried immediately */
        close(sv[0]); close(ctl[0]);
        int fl = fcntl(sv[1], F_GETFL, 0);
        if (fl >= 0) fcntl(sv[1], F_SETFL, fl | O_NONBLOCK);
        u64 *t = malloc(P5B_N * sizeof(u64));
        if (!t) _exit(42);
        char buf[65536];
        size_t n = 0;
        u64 eagain = 0;
        for (;;) {
            u64 t0 = rdtsc_();
            ssize_t r = read(sv[1], buf, sizeof(buf));
            u64 t1 = rdtsc_();
            if (r == 0) break;                 /* EOF: parent closed */
            if (r < 0) {
                if (errno == EAGAIN) { eagain++; continue; }
                break;
            }
            if (n < P5B_N) t[n++] = t1 - t0;   /* successful drain cost */
        }
        struct child_sum cs;
        cs.n = n;
        stats_of(t, n, &cs.s);
        cs.wall_ns = 0;
        cs.errno_hit = eagain;                 /* eagain spin count */
        if (write(ctl[1], &cs, sizeof(cs)) != (ssize_t)sizeof(cs)) _exit(43);
        _exit(0);
    }
    close(sv[1]); close(ctl[1]);
    u64 *t = xmalloc_samples(P5C_N);
    if (!t) { close(sv[0]); close(ctl[0]); waitpid(pid, 0, 0); return; }
    char rec[REC_BYTES];
    memset(rec, 0xe7, sizeof(rec));
    for (int i = 0; i < 2000; i++) { if (sendto(sv[0], rec, REC_BYTES, 0, 0, 0) != REC_BYTES) break; }
    size_t n = 0;
    s64 w = mnr_ns();
    for (int i = 0; i < P5C_N; i++) {
        u64 t0 = rdtsc_();
        ssize_t r = sendto(sv[0], rec, REC_BYTES, 0, 0, 0);
        u64 t1 = rdtsc_();
        if (r != REC_BYTES) break;
        t[n++] = t1 - t0;
    }
    w = mnr_ns() - w;
    close(sv[0]);                              /* EOF ends the child's spin */
    struct child_sum cs;
    ssize_t rr = read(ctl[0], &cs, sizeof(cs));
    int st; waitpid(pid, &st, 0);
    report_ticks("p5e_stream_send", t, n, w, n < (size_t)P5C_N ? "short=1" : "");
    free(t);
    if (rr == (ssize_t)sizeof(cs) && cs.n) {
        char note[64];
        snprintf(note, sizeof(note), "measured_by=child spin-read eagain=%llu",
                 (unsigned long long)cs.errno_hit);
        report_s("p5e_child_spin", cs.n, &cs.s, 0, note);
    } else {
        skip_line("p5e_child_spin", "child-summary-lost");
    }
    close(ctl[0]);
}

/* p5f (leg 46): p5c2's exact child (blocking read(2) drain) but the
 * parent NEVER SLEEPS: sendto(MSG_DONTWAIT), and on EAGAIN retry
 * immediately (pure spin). The measured samples are the SUCCESSFUL
 * sendto calls only; the eagain count in the note is the spin density.
 *   p5f med ~fast && eagain high => the queue does fill (a drain-rate
 *     bound exists) but p5c2's 40 us median is the parent's own SLEEP+
 *     WAKE cycle cost — sender-side wake->pick is the lever;
 *   p5f med ~40 us => the per-success spacing is set by drain
 *     throughput itself — the child's drain cycle (its wake->drain
 *     latency or per-call cost) owns the class. */
static void b_p5f(void)
{
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        skip_line("p5f_stream_dwait", "socketpair-stream-errno");
        return;
    }
    pid_t pid = fork();
    if (pid < 0) { skip_line("p5f_stream_dwait", "fork-errno"); close(sv[0]); close(sv[1]); return; }
    if (pid == 0) {
        close(sv[0]);
        char buf[65536];
        while (read(sv[1], buf, sizeof(buf)) > 0) ;
        _exit(0);
    }
    close(sv[1]);
    u64 *t = xmalloc_samples(P5C_N);
    if (!t) { close(sv[0]); waitpid(pid, 0, 0); return; }
    char rec[REC_BYTES];
    memset(rec, 0xe7, sizeof(rec));
    for (int i = 0; i < 2000; i++) {
        if (sendto(sv[0], rec, REC_BYTES, MSG_DONTWAIT, 0, 0) != REC_BYTES) break;
    }
    size_t n = 0;
    u64 eagain = 0;
    u64 attempts = 0;
    const u64 max_attempts = (u64)P5C_N * 64;   /* wedge guard */
    s64 w = mnr_ns();
    while (n < (size_t)P5C_N && attempts < max_attempts) {
        attempts++;
        u64 t0 = rdtsc_();
        ssize_t r = sendto(sv[0], rec, REC_BYTES, MSG_DONTWAIT, 0, 0);
        u64 t1 = rdtsc_();
        if (r == REC_BYTES) { t[n++] = t1 - t0; continue; }
        if (r < 0 && errno == EAGAIN) { eagain++; continue; }
        break;                                  /* real error: stop */
    }
    w = mnr_ns() - w;
    close(sv[0]);
    waitpid(pid, 0, 0);
    char note[96];
    snprintf(note, sizeof(note), "attempts=%llu eagain=%llu (%.1f%% spin)",
             (unsigned long long)attempts, (unsigned long long)eagain,
             attempts ? 100.0 * (double)eagain / (double)attempts : 0.0);
    report_ticks("p5f_stream_dwait", t, n, w, note);
    free(t);
}

/* p5c2long/p5c3long (leg 46): duration-bounded windows of the exact
 * p5c2/p5c3 shapes for the dfx=wwcensus boot — PBENCH marker lines
 * bracket each window so the serial log's WW heartbeat lines (1 Hz,
 * wakes/picks/avg_run/stranded) can be differenced inside the slow
 * window (read-child) against the fast window (recvfrom-child) and the
 * idle gaps between them. Samples past P5C_N are not recorded; the
 * syscall shapes run unchanged for the full window.
 * parent_cpu/child_cpu: -1 = unpinned; else sched_setaffinity to that
 * CPU BEFORE the loop (leg 46 placement discriminator: pa72e pinned
 * both tasks to CPU0 and reproduced the 40-60 us class on demand
 * (med 59.4) while spread/unpinned ran 7.8-8.4 — the class IS the
 * serialized same-CPU handoff shape).
 * child_buf: the drain syscall's buffer size — the receive-path staging
 * discriminator. The stock children drain with 64KB reads/recvfroms,
 * so every drain call stages a 64KB kernel buffer (alloc+copy+free);
 * a 4KB drain buffer cuts the staging 16x without changing the
 * sleep/wake shape. If the samecpu per-send cost collapses with the
 * small buffer, the staging owns the serialized cycle. */
static void b_p5_long(int child_recv, int secs, const char *tag,
                      int parent_cpu, int child_cpu, int child_buf)
{
    char tagline[96];
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        snprintf(tagline, sizeof(tagline), "PBENCH %s SKIP why=socketpair-errno\n", tag);
        emit(tagline);
        return;
    }
    pid_t pid = fork();
    if (pid < 0) {
        snprintf(tagline, sizeof(tagline), "PBENCH %s SKIP why=fork-errno\n", tag);
        emit(tagline);
        close(sv[0]); close(sv[1]);
        return;
    }
    if (pid == 0) {
        close(sv[0]);
        if (child_cpu >= 0) {
            cpu_set_t set;
            CPU_ZERO(&set);
            CPU_SET(child_cpu, &set);
            sched_setaffinity(0, sizeof(set), &set);
        }
        char buf[65536];
        if (child_recv) { while (recvfrom(sv[1], buf, (size_t)child_buf, 0, 0, 0) > 0) ; }
        else            { while (read(sv[1], buf, (size_t)child_buf) > 0) ; }
        _exit(0);
    }
    close(sv[1]);
    if (parent_cpu >= 0) {
        cpu_set_t set;
        CPU_ZERO(&set);
        CPU_SET(parent_cpu, &set);
        sched_setaffinity(0, sizeof(set), &set);
    }
    u64 *t = xmalloc_samples(P5C_N);
    if (!t) { close(sv[0]); waitpid(pid, 0, 0); return; }
    char rec[REC_BYTES];
    memset(rec, 0xa3, sizeof(rec));
    for (int i = 0; i < 2000; i++) {
        if (sendto(sv[0], rec, REC_BYTES, 0, 0, 0) != REC_BYTES) break;
    }
    snprintf(tagline, sizeof(tagline),
             "PBENCH %s begin secs=%d pcpu=%d ccpu=%d\n", tag, secs,
             parent_cpu, child_cpu);
    emit(tagline);
    size_t n = 0;
    u64 iters = 0;
    s64 w = mnr_ns();
    s64 target = w + (s64)secs * 1000000000LL;
    for (;;) {
        u64 t0 = rdtsc_();
        ssize_t r = sendto(sv[0], rec, REC_BYTES, 0, 0, 0);
        u64 t1 = rdtsc_();
        if (r != REC_BYTES) break;
        iters++;
        if (n < (size_t)P5C_N) t[n++] = t1 - t0;
        if (mnr_ns() >= target) break;
    }
    w = mnr_ns() - w;
    snprintf(tagline, sizeof(tagline), "PBENCH %s end iters=%llu\n", tag,
             (unsigned long long)iters);
    emit(tagline);
    close(sv[0]);
    waitpid(pid, 0, 0);
    report_ticks(tag, t, n, w, "");
    free(t);
    if (parent_cpu >= 0) {   /* unpin for the rows that follow */
        cpu_set_t set;
        CPU_ZERO(&set);
        CPU_SET(0, &set); CPU_SET(1, &set);
        sched_setaffinity(0, sizeof(set), &set);
    }
}

/* ---------------- P6: fs ops on an identical tree ---------------- */
static void p6_path(char *out, size_t outsz, const char *fmt, long i)
{
    snprintf(out, outsz, "%s/pbench-tree/%s", g_work, fmt);
    char *pct = strrchr(out, '%');
    if (pct) snprintf(pct, outsz - (size_t)(pct - out), "%04ld", i);
}

static int p6_build_tree(void)
{
    char dir[300], path[300];
    snprintf(dir, sizeof(dir), "%s/pbench-tree", g_work);
    if (mkdir(dir, 0755) < 0 && errno != EEXIST) return -1;
    char data[4096];
    u32 seed = 999;
    for (size_t i = 0; i < sizeof(data); i++) { seed = seed * 1103515245 + 12345; data[i] = (char)(seed >> 16); }
    for (long i = 0; i < P6_FILES; i++) {
        p6_path(path, sizeof(path), "f%", i);
        int fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
        if (fd < 0) return -1;
        if (write(fd, data, sizeof(data)) != (ssize_t)sizeof(data)) { close(fd); return -1; }
        close(fd);
    }
    return 0;
}

struct mini_dirent { u64 d_ino; s64 d_off; unsigned short d_reclen; unsigned char d_type; char d_name[]; };

static void b_p6(void)
{
    s64 w0 = mnr_ns();
    if (p6_build_tree() < 0) {
        skip_line("p6_stat", "tree-build-errno");
        return;
    }
    s64 w1 = mnr_ns();
    {
        char b[160];
        int len = snprintf(b, sizeof(b), "PBENCH PREP tree=%ldfiles wall_ms=%lld\n",
                           (long)P6_FILES, (w1 - w0) / 1000000);
        if (len > 0) emit_len(b, (size_t)len);
    }
    char path[300];

    /* stat */
    {
        u64 *t = xmalloc_samples((size_t)P6_FILES * P6_ROUNDS);
        if (t) {
            struct stat st;
            size_t n = 0;
            s64 w = mnr_ns();
            for (int r = 0; r < P6_ROUNDS + 1; r++) {
                int live = r > 0;         /* first round = warmup */
                size_t base = live ? n : 0;
                for (long i = 0; i < P6_FILES; i++) {
                    p6_path(path, sizeof(path), "f%", i);
                    u64 t0 = rdtsc_();
                    stat(path, &st);
                    u64 t1 = rdtsc_();
                    if (live) t[base + (size_t)i] = t1 - t0;
                }
                if (live) n += P6_FILES;
            }
            w = mnr_ns() - w;
            report_ticks("p6_stat", t, n, w, "");
            free(t);
        }
    }

    /* open+close */
    {
        u64 *t = xmalloc_samples((size_t)P6_FILES * P6_ROUNDS);
        if (t) {
            size_t n = 0;
            s64 w = mnr_ns();
            for (int r = 0; r < P6_ROUNDS + 1; r++) {
                int live = r > 0;
                size_t base = live ? n : 0;
                for (long i = 0; i < P6_FILES; i++) {
                    p6_path(path, sizeof(path), "f%", i);
                    u64 t0 = rdtsc_();
                    int fd = open(path, O_RDONLY);
                    if (fd >= 0) close(fd);
                    u64 t1 = rdtsc_();
                    if (live) t[base + (size_t)i] = t1 - t0;
                }
                if (live) n += P6_FILES;
            }
            w = mnr_ns() - w;
            report_ticks("p6_openclose", t, n, w, "");
            free(t);
        }
    }

    /* getdents64 full-dir sweep */
    {
        u64 *t = xmalloc_samples(1000);
        if (t) {
            snprintf(path, sizeof(path), "%s/pbench-tree", g_work);
            size_t n = 0;
            s64 w = mnr_ns();
            for (int r = 0; r < 1001; r++) {
                int live = r > 0;
                int fd = open(path, O_RDONLY | O_DIRECTORY);
                if (fd < 0) break;
                char buf[32768];
                u64 t0 = rdtsc_();
                for (;;) {
                    ssize_t k = syscall(SYS_getdents64, fd, buf, sizeof(buf));
                    if (k <= 0) break;
                }
                u64 t1 = rdtsc_();
                close(fd);
                if (live && n < 1000) t[n++] = t1 - t0;
            }
            w = mnr_ns() - w;
            report_ticks("p6_getdents", t, n, w, "per_sample=full_dir_sweep");
            free(t);
        }
    }

    /* page-cached read: open+read 4KB+close */
    {
        u64 *t = xmalloc_samples((size_t)P6_FILES * P6_ROUNDS);
        if (t) {
            char data[4096];
            size_t n = 0;
            s64 w = mnr_ns();
            for (int r = 0; r < P6_ROUNDS + 1; r++) {
                int live = r > 0;
                size_t base = live ? n : 0;
                for (long i = 0; i < P6_FILES; i++) {
                    p6_path(path, sizeof(path), "f%", i);
                    u64 t0 = rdtsc_();
                    int fd = open(path, O_RDONLY);
                    if (fd >= 0) {
                        if (read(fd, data, sizeof(data)) == (ssize_t)sizeof(data)) ;
                        close(fd);
                    }
                    u64 t1 = rdtsc_();
                    if (live) t[base + (size_t)i] = t1 - t0;
                }
                if (live) n += P6_FILES;
            }
            w = mnr_ns() - w;
            report_ticks("p6_read_cached", t, n, w, "per_sample=open_read_close");
            free(t);
        }
    }

    /* 4 KB write + fsync */
    {
        u64 *t = xmalloc_samples(P6_SYNC_N);
        if (t) {
            snprintf(path, sizeof(path), "%s/pbench-sync.bin", g_work);
            char data[4096];
            memset(data, 0xc3, sizeof(data));
            int fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
            if (fd < 0) {
                skip_line("p6_write_fsync", "open-create-errno");
            } else {
                write(fd, data, sizeof(data)); fsync(fd); /* warmup */
                size_t n = 0;
                s64 w = mnr_ns();
                for (int i = 0; i < P6_SYNC_N; i++) {
                    u64 t0 = rdtsc_();
                    if (write(fd, data, sizeof(data)) != (ssize_t)sizeof(data)) break;
                    if (fsync(fd) < 0) break;
                    u64 t1 = rdtsc_();
                    t[n++] = t1 - t0;
                }
                w = mnr_ns() - w;
                close(fd);
                report_ticks("p6_write_fsync", t, n, w, "per_sample=write4k_plus_fsync");
            }
            free(t);
        }
    }

    /* append */
    {
        u64 *t = xmalloc_samples(P6_SYNC_N);
        if (t) {
            snprintf(path, sizeof(path), "%s/pbench-append.bin", g_work);
            char data[4096];
            memset(data, 0x3c, sizeof(data));
            int fd = open(path, O_CREAT | O_WRONLY | O_APPEND, 0644);
            if (fd < 0) {
                skip_line("p6_append", "open-create-errno");
            } else {
                write(fd, data, sizeof(data)); /* warmup */
                size_t n = 0;
                s64 w = mnr_ns();
                for (int i = 0; i < P6_SYNC_N; i++) {
                    u64 t0 = rdtsc_();
                    if (write(fd, data, sizeof(data)) != (ssize_t)sizeof(data)) break;
                    u64 t1 = rdtsc_();
                    t[n++] = t1 - t0;
                }
                w = mnr_ns() - w;
                close(fd);
                report_ticks("p6_append", t, n, w, "");
            }
            free(t);
        }
    }

    /* mkdir+rmdir (pair per sample) */
    {
        u64 *t = xmalloc_samples(P6_SYNC_N);
        if (t) {
            size_t n = 0;
            s64 w = mnr_ns();
            mkdir(g_work, 0755); /* ensure base (already exists) */
            for (int i = 0; i < P6_SYNC_N + 1; i++) {
                int live = i > 0;
                p6_path(path, sizeof(path), "d%", i);
                u64 t0 = rdtsc_();
                int mk = mkdir(path, 0755);
                int rm = (mk == 0) ? rmdir(path) : -1;
                u64 t1 = rdtsc_();
                (void)rm;
                if (live) t[n++] = t1 - t0;
                if (mk < 0) break;
            }
            w = mnr_ns() - w;
            report_ticks("p6_mkdrmdir", t, n, w, "per_sample=mkdir_plus_rmdir");
            free(t);
        }
    }

    /* unlink (one per tree file, destroys the tree — last) */
    {
        u64 *t = xmalloc_samples(P6_FILES);
        if (t) {
            size_t n = 0;
            s64 w = mnr_ns();
            for (long i = 0; i < P6_FILES; i++) {
                p6_path(path, sizeof(path), "f%", i);
                u64 t0 = rdtsc_();
                if (unlink(path) < 0) break;
                u64 t1 = rdtsc_();
                t[n++] = t1 - t0;
            }
            w = mnr_ns() - w;
            report_ticks("p6_unlink", t, n, w, "");
            free(t);
        }
    }
}

/* ---------------- P7: sleep overshoot ---------------- */
static void b_p7(void)
{
    u64 *t = xmalloc_samples(P7_N);
    if (!t) return;
    struct timespec req = { 0, 1000000 }, rem;
    for (int i = 0; i < 100; i++) nanosleep(&req, &rem);
    size_t n = 0;
    s64 w = mnr_ns();
    for (int i = 0; i < P7_N; i++) {
        s64 c0 = mnr_ns();
        nanosleep(&req, &rem);
        s64 c1 = mnr_ns();
        s64 ovh = c1 - c0 - 1000000LL;
        if (ovh < 0) ovh = 0;
        t[n++] = (u64)ovh;
    }
    w = mnr_ns() - w;
    report_ticks("p7_sleep1ms", t, n, w, "req=1ms val=overshoot");
    free(t);
}

/* ---------------- main ---------------- */
int main(int argc, char **argv)
{
    if (argc > 1 && strcmp(argv[1], "-x") == 0) _exit(0); /* P4 exec target */

    if (argc > 1 && strcmp(argv[1], "longp5") == 0) {
        /* leg 46 census mode: duration-bounded p5c2/p5c3 windows with
         * serial markers, for boots whose cmdline carries dfx=wwcensus
         * (the 1 Hz WW heartbeat + wake-refusal prints between the
         * markers are the measurement). Staged via /test/p5long. */
        signal(SIGPIPE, SIG_IGN);
        g_out_fd = open("/test/pbench-results.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
        emit("PBENCH START mode=longp5\n");
        calibrate();
        b_p5_long(0, 12, "p5c2long", -1, -1, 65536);
        b_p5_long(1, 12, "p5c3long", -1, -1, 65536);
        emit("PBENCH DONE rc=0\n");
        if (g_out_fd >= 0) close(g_out_fd);
        syscall(SYS_sync);
        syscall(SYS_reboot, 0xfee1dead, 672274793, 0x4321FEDC); /* POWER_OFF */
        return 0;
    }

    if (argc > 1 && strcmp(argv[1], "longp5warm") == 0) {
        /* leg 46 census mode 2: run the board's p5 section FIRST (the
         * regime driver — pa72b's board read p5b/p5c/p5c2/p5f all in
         * the 39-40 us class while pa72c's cold windows ran the same
         * shapes at 4.9-5.2), then the long windows: unpinned p5c2/
         * p5c3, then the placement discriminators (parent/child pinned
         * spread 0/1, then both pinned to CPU0). Boot with
         * dfx=wwcensus; every window is bracketed by markers. */
        signal(SIGPIPE, SIG_IGN);
        g_out_fd = open("/test/pbench-results.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
        emit("PBENCH START mode=longp5warm\n");
        calibrate();
        b_p5a();
        b_p5b();
        b_p5c();
        b_p5c2();
        b_p5c3();
        b_p5_long(0, 12, "p5c2long", -1, -1, 65536);
        b_p5_long(1, 12, "p5c3long", -1, -1, 65536);
        b_p5_long(0, 12, "p5c2long_spread", 0, 1, 65536);
        b_p5_long(0, 12, "p5c2long_samecpu", 0, 0, 65536);
        emit("PBENCH DONE rc=0\n");
        if (g_out_fd >= 0) close(g_out_fd);
        syscall(SYS_sync);
        syscall(SYS_reboot, 0xfee1dead, 672274793, 0x4321FEDC); /* POWER_OFF */
        return 0;
    }

    if (argc > 1 && strcmp(argv[1], "longp5pin") == 0) {
        /* leg 46 census mode 3: the serialized-cycle discriminators.
         * pa72e established samecpu pinning reproduces the 40-60 us
         * class on demand (wakes==picks census: direct handoff per
         * cycle). These windows decompose the per-cycle cost under
         * that regime:
         *   samecpu read 64KB  — the anchor (expected ~59 us)
         *   samecpu read 4KB   — staging discriminator: 16x smaller
         *                        per-drain staging, same sleep/wake
         *                        shape; collapse => staging owns it
         *   samecpu recv 64KB  — syscall-agnostic check (recvfrom
         *                        stages 64KB too)
         *   spread read 64KB   — the concurrent control (~8 us) */
        signal(SIGPIPE, SIG_IGN);
        g_out_fd = open("/test/pbench-results.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
        emit("PBENCH START mode=longp5pin\n");
        calibrate();
        b_p5_long(0, 12, "pin_r64", 0, 0, 65536);
        b_p5_long(0, 12, "pin_r4k", 0, 0, 4096);
        b_p5_long(1, 12, "pin_recv64", 0, 0, 65536);
        b_p5_long(0, 12, "spread_r64", 0, 1, 65536);
        emit("PBENCH DONE rc=0\n");
        if (g_out_fd >= 0) close(g_out_fd);
        syscall(SYS_sync);
        syscall(SYS_reboot, 0xfee1dead, 672274793, 0x4321FEDC); /* POWER_OFF */
        return 0;
    }

    if (argc > 1 && argv[1][0] != '-') {
        snprintf(g_work, sizeof(g_work), "%s", argv[1]);
        if (g_work[0] != '/') snprintf(g_work, sizeof(g_work), "/test");
    }
    if (argc > 2 && argv[2][0] == '/') {
        g_out_fd = open(argv[2], O_WRONLY | O_CREAT | O_TRUNC, 0644);
    } else {
        char rf[300];
        snprintf(rf, sizeof(rf), "%s/pbench-results.txt", g_work);
        g_out_fd = open(rf, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    }
    if (argv[0] && argv[0][0] == '/')
        snprintf(g_self, sizeof(g_self), "%s", argv[0]);

    signal(SIGPIPE, SIG_IGN);

    {
        char b[160];
        int len = snprintf(b, sizeof(b), "PBENCH START self=%s work=%s\n", g_self, g_work);
        if (len > 0) emit_len(b, (size_t)len);
    }

    calibrate();
    b_p1();
    b_p2();
    b_p3a();
    b_p3b();
    {
        char fpath[300];
        snprintf(fpath, sizeof(fpath), "%s/pbench-faultfile.bin", g_work);
        b_p3c(fpath);
        b_p3d(g_work);
    }
    b_p4();
    b_p5a();
    b_p5b();
    b_p5c();
    b_p5c2();
    b_p5c3();
    b_p5d_read();
    b_p5d_recv();
    b_p5e();
    b_p5f();
    b_p6();
    b_p7();

    emit("PBENCH DONE rc=0\n");
    if (g_out_fd >= 0) close(g_out_fd);
    syscall(SYS_sync);
    syscall(SYS_reboot, 0xfee1dead, 672274793, 0x4321FEDC); /* POWER_OFF */
    return 0; /* if reboot failed */
}
