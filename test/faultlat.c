/*
 * faultlat.c — file-backed page-fault latency probe.
 *
 * The OH boots show large-.so loads crawling at ~1.5 s per file-backed
 * page fault (SAMGR: SA:3002 OpenSo 802166ms, media_service in D-state
 * the whole window). This probe reproduces the access pattern minimally:
 * mmap a preallocated data file read-only and fault its pages in
 * sequentially, reporting per-page wall time (min/avg/max via
 * clock_gettime MONOTONIC) plus the plain read() throughput of the same
 * file for comparison.
 *
 * Prints lines like:
 *   FAULTLAT mmap 8192 pages: first=...us avg=...us max=...us total=...ms
 *   FAULTLAT read 32768 KiB in ... ms (... MiB/s)
 *   FAULTLAT RESULT: SLOW|FAST avg_page_us=<n>
 * (SLOW = avg page fault above 100000 us.)
 *
 * Build: like drm_probe.c (static musl, see faultlat_boot.py).
 */
#include <fcntl.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
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

#define DATA "/test/bigdata.bin"

static inline unsigned long rng(unsigned long *s) {
    *s ^= *s << 13; *s ^= *s >> 7; *s ^= *s << 17;
    return *s;
}

int main(void) {
    int fd = open(DATA, O_RDONLY);
    if (fd < 0) { puts_("FAULTLAT open " DATA " failed\n"); return 1; }
    struct stat st;
    if (fstat(fd, &st) < 0 || st.st_size < 4096) {
        puts_("FAULTLAT stat bad\n"); return 1;
    }
    size_t len = (size_t)st.st_size;

    /* ---- mmap fault sweep ---- */
    char *p = mmap(0, len, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED) { puts_("FAULTLAT mmap failed\n"); return 1; }
    size_t npages = len / 4096;
    unsigned long seed = 0x12345678U;
    volatile unsigned long sink = 0;

    long long t_first = -1, t_total0 = now_us();
    unsigned long worst = 0; unsigned long long acc = 0;
    for (size_t i = 0; i < npages; i++) {
        long long t0 = now_us();
        sink += p[i * 4096];
        long long dt = now_us() - t0;
        if (t_first < 0) t_first = dt;
        acc += (unsigned long)dt;
        if ((unsigned long)dt > worst) worst = (unsigned long)dt;
        /* touch a second word in some pages so the compiler cannot fold */
        if ((rng(&seed) & 7) == 0) sink += p[i * 4096 + 64];
    }
    long long t_total = now_us() - t_total0;

    puts_("FAULTLAT mmap "); putu_(npages); puts_(" pages: first=");
    putu_((unsigned long)t_first); puts_("us avg=");
    putu_((unsigned long)(acc / (npages ? npages : 1))); puts_("us max=");
    putu_(worst); puts_("us total="); putu_((unsigned long)t_total);
    puts_("ms sink="); putu_(sink & 0xffff); puts_("\n");

    munmap(p, len);

    /* ---- sequential read sweep (same file, fresh page cache is NOT
     * evicted — this measures the warm read() path) ---- */
    long long r0 = now_us();
    char buf[65536];
    ssize_t n; size_t tot = 0;
    lseek(fd, 0, SEEK_SET);
    while ((n = read(fd, buf, sizeof(buf))) > 0) tot += (size_t)n;
    long long rdt = now_us() - r0;
    puts_("FAULTLAT read "); putu_(tot / 1024); puts_(" KiB in ");
    putu_((unsigned long)rdt / 1000); puts_("ms (");
    if (rdt > 0 && tot) { putu_((unsigned long)(tot / 1024 / 1024 * 1000000ULL / (unsigned long)rdt)); puts_(" MiB/s"); }
    else puts_("?"); puts_("\n");

    unsigned long avg = (unsigned long)(acc / (npages ? npages : 1));
    puts_("FAULTLAT RESULT: ");
    if (avg > 100000UL) { puts_("SLOW"); } else { puts_("FAST"); }
    puts_(" avg_page_us="); putu_(avg); puts_("\n");
    return 0;
}
