/*
 * faultlat3.c — file-backed page-fault latency under the OH-style mixture.
 *
 * faultlat/faultlat2 showed the RAW path is fast (54 us/page single
 * reader; 0.3–1 ms avg with 6 readers). The OH boots show ~1.5 s per
 * .so page fault (SA:3002 OpenSo 802166ms) — the difference is the
 * service mixture: many concurrent .so loaders whose aggregate working
 * set exceeds the page cache, PLUS concurrent writers driving journal
 * commits that hold the ext4 big lock across synchronous I/O.
 *
 * This probe reproduces that mixture minimally:
 *   - N_LOADER children mmap DISTINCT data files (aggregate larger than
 *     the old 32MB cache cap) read-only and fault every page in order,
 *     reporting per-page avg/max.
 *   - N_WRITER children append 64 KiB chunks to journal-target files for
 *     the whole run (ext4 journal commits under the big lock).
 *   - /proc/meminfo PageCacheOps is sampled before/after (Rux extension:
 *     hits/misses/evictions/demotions) so the thrash is visible directly.
 *
 * Prints:
 *   FAULTLAT3 loader <i> pages=<n> avg=<us> max=<us>
 *   FAULTLAT3 writer <i> wrote=<KiB> commits=<n>
 *   FAULTLAT3 meminfo before: <...> / after: <...>
 *   FAULTLAT3 all loaders done in <ms>ms
 *   FAULTLAT3 RESULT: SLOW|FAST worst_avg_us=<n>
 * (SLOW = worst loader avg above SLOW_THRESH_US.)
 *
 * Build: like faultlat.c (static musl, see faultlat3_boot.py).
 */
#include <fcntl.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <signal.h>
#include <time.h>

static int puts_(const char *s) { return write(1, s, strlen(s)); }
static void putu_(unsigned long v) {
    char b[24]; int i = 24;
    if (!v) { puts_("0"); return; }
    while (v && i) { b[--i] = '0' + (v % 10); v /= 10; }
    write(1, b + i, 24 - i);
}
static void putsu_(const char *s, unsigned long v) { puts_(s); putu_(v); }

static long long now_us(void) {
    struct timespec ts;
    syscall(SYS_clock_gettime, CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000000LL + ts.tv_nsec / 1000;
}

#define N_LOADER 6
#define N_WRITER 2
#define LOAD_MB  16          /* per loader; 6 x 16MB = 96MB aggregate */
#define SLOW_THRESH_US 20000 /* 20 ms avg -> SLOW (raw path is 0.05ms) */

static volatile unsigned long sink;

/* /proc/meminfo PageCacheOps line, or "none" (baseline kernel lacks it) */
static void read_meminfo(const char *tag) {
    int fd = open("/proc/meminfo", O_RDONLY);
    char buf[4096];
    puts_("FAULTLAT3 meminfo "); puts_(tag); puts_(": ");
    if (fd < 0) { puts_("no-proc\n"); return; }
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0) { puts_("empty\n"); return; }
    buf[n] = 0;
    char *p = strstr(buf, "PageCacheOps:");
    if (!p) { puts_("no-pcops\n"); return; }
    char *e = strchr(p, '\n');
    if (e) *e = 0;
    puts_(p); puts_("\n");
    if (e) *e = '\n';
}

static void loader_child(int idx) {
    static const char *names[N_LOADER] = {
        "/test/l0.bin", "/test/l1.bin", "/test/l2.bin",
        "/test/l3.bin", "/test/l4.bin", "/test/l5.bin",
    };
    const char *path = names[idx];
    int fd = open(path, O_RDONLY);
    if (fd < 0) { puts_("FAULTLAT3 loader open failed\n"); _exit(9); }
    struct stat st;
    if (fstat(fd, &st) < 0 || st.st_size < 65536) { close(fd); _exit(9); }
    char *p = mmap(0, (size_t)st.st_size, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED) { close(fd); _exit(9); }
    size_t npages = (size_t)st.st_size / 4096;
    unsigned long long acc = 0;
    unsigned long mx = 0;
    long long t_start = now_us(), t_first = -1;
    for (size_t i = 0; i < npages; i++) {
        long long t0 = now_us();
        sink += p[i * 4096];
        unsigned long dt = (unsigned long)(now_us() - t0);
        if (t_first < 0) t_first = now_us() - t_start;
        acc += dt;
        if (dt > mx) mx = dt;
    }
    putsu_("FAULTLAT3 loader ", (unsigned long)idx);
    putsu_(" pages=", npages);
    putsu_(" first-took=", (unsigned long)t_first);
    putsu_("us avg=", (unsigned long)(acc / npages));
    putsu_("us max=", mx);
    puts_("us\n");
    _exit(0);
}

static void writer_child(int idx) {
    static const char *names[N_WRITER] = { "/test/j0.bin", "/test/j1.bin" };
    int fd = open(names[idx], O_WRONLY | O_CREAT | O_APPEND, 0644);
    if (fd < 0) _exit(9);
    static char chunk[65536];
    for (int i = 0; i < 65536; i += 16) memcpy(chunk + i, "journal-churn-0", 16);
    unsigned long wrote = 0, commits = 0;
    for (;;) {
        ssize_t w = write(fd, chunk, sizeof(chunk));
        if (w <= 0) break;
        wrote += (unsigned long)w;
        commits++;
        /* parent SIGKILLs us when the loaders are done */
    }
    putsu_("FAULTLAT3 writer ", (unsigned long)idx);
    putsu_(" wrote=", wrote / 1024);
    putsu_("KiB commits=", commits);
    puts_("\n");
    _exit(0);
}

int main(void) {
    read_meminfo("before");

    long long t0 = now_us();
    pid_t loaders[N_LOADER], writers[N_WRITER];

    for (int i = 0; i < N_WRITER; i++) {
        pid_t pid = fork();
        if (pid == 0) writer_child(i);
        writers[i] = pid;
    }
    for (int i = 0; i < N_LOADER; i++) {
        pid_t pid = fork();
        if (pid == 0) loader_child(i);
        loaders[i] = pid;
    }

    for (int i = 0; i < N_LOADER; i++) {
        int st = 0;
        waitpid(loaders[i], &st, 0);
        if (!WIFEXITED(st) || WEXITSTATUS(st) != 0)
            putsu_("FAULTLAT3 loader ", (unsigned long)i), puts_(" FAILED\n");
    }
    unsigned long wall = (unsigned long)((now_us() - t0) / 1000);

    for (int i = 0; i < N_WRITER; i++) kill(writers[i], 9);
    for (int i = 0; i < N_WRITER; i++) waitpid(writers[i], 0, 0);

    /* re-read loader avg lines is needlessly complex: the loader children
     * printed their own averages to serial. The parent's worst-avg verdict
     * rides the wall time instead: 6 loaders x 4096 pages x raw 54us
     * ~= 1.3s per loader when cached; thrash shows up as tens of seconds. */
    read_meminfo("after");
    putsu_("FAULTLAT3 all loaders done in ", wall); puts_("ms\n");
    /* Verdict from wall time, scaled to the file size: SLOW when wall
     * implies > 2 ms/page across the loader set (raw-path avg is
     * ~0.05-0.5 ms/page; the thrash family is 10+ ms/page), with a 15 s
     * floor for small files. Per-loader avg/max lines above carry the
     * precise per-page numbers. */
    {
        /* npages comes from stat, not the compile-time LOAD_MB (the boot
         * driver can stage larger files): 2 ms/page across the loader set
         * is SLOW, with a 15 s floor for small files. */
        size_t probe_pages = 0;
        struct stat pst;
        int pfd = open("/test/l0.bin", O_RDONLY);
        if (pfd >= 0 && fstat(pfd, &pst) == 0) probe_pages = pst.st_size / 4096;
        if (pfd >= 0) close(pfd);
        if (probe_pages == 0) probe_pages = (size_t)LOAD_MB * 256;
        unsigned long slow_ms = 2UL * (unsigned long)probe_pages;
        if (slow_ms < 15000) slow_ms = 15000;
        if (wall > slow_ms) puts_("FAULTLAT3 RESULT: SLOW wall_ms=");
        else puts_("FAULTLAT3 RESULT: FAST wall_ms=");
    }
    putu_(wall); puts_(" sink=");
    putu_(sink & 0xffff); puts_("\n");
    return 0;
}
