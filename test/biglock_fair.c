// biglock_fair.c — EXT4_BIG_LOCK fairness probe (the b139 starvation shape).
//
// Reproduces, without OpenHarmony, the oh-b139 evidence: many tenants
// driving page-cache MISS reads through ext4 (file fault / first-time
// .so load path: miss -> InodePageCache::search_tree/fill -> big lock)
// while a low-frequency canary reader must keep bounded latency.
// On the pre-R84 kernel the canary (and whole services, in b139) sat
// parked in lock_fair for seconds while fresh fast-path callers barged
// past the woken waiter (EXT4-BIGLOCK-STALL held=0j).
//
// Topology (one process, SMP2 guest):
//   - setup: 6 corpus files x 6 MB (36 MB total > the 32 MB page-cache
//     cap, so the rotating readers MISS constantly), 1 canary file
//     (4 MB, rotated so its pages go cold), 1 mmap file (6 MB).
//   - 6 hammer threads: pread() 4 KB at rotating offsets over their own
//     corpus file (lock-free cache hits miss the point; rotation across
//     an over-cap working set keeps the miss path hot).
//   - 1 churn thread: create+write+read+unlink a 64 KB file in a loop
//     (the /data first-boot metadata shape: namei + write under the
//     same big lock).
//   - 1 faulter thread: mmap one corpus file MAP_PRIVATE and touch
//     pages round-robin (the .so fault path; LRU churn re-evicts
//     behind it so refaults keep missing).
//   - 1 canary thread: every 100 ms, one 4 KB pread at a rotating
//     offset of the canary file, latency recorded. THIS is the
//     assertion: max latency must stay under the bound.
//   - main: progress line every 5 s, verdict after <secs>.
//
// Verdict:
//   BIGFAIR-PASS   canary max < bound (and no stall print seen)
//   BIGFAIR-FAIL   canary max >= bound
// Driver (run/bigfair.py) also scans the serial log for
// EXT4-BIGLOCK-STALL and fails the run if any appear.
//
// Usage: biglock_fair [seconds=60] [bound_ms=2000]
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>

#define NHAMMER 6
#define CORPUS_MB 6
#define CANARY_MB 4
#define CHURN_KB 64
#define CANARY_PERIOD_MS 100

static volatile int stop_flag;
static unsigned long hammer_ops, churn_ops, fault_ops;
static unsigned long canary_n, canary_over500, canary_over1s;
static unsigned long canary_max_ms;
static unsigned long bound_ms = 2000;

static double now_ms(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1000.0 + (double)ts.tv_nsec / 1e6;
}

static void fill_pattern(unsigned char *buf, size_t len, unsigned seed)
{
    unsigned x = seed * 2654435761u + 1;
    for (size_t i = 0; i < len; i += 4) {
        x ^= x << 13; x ^= x >> 17; x ^= x << 5;
        buf[i] = (unsigned char)(x >> 24);
        if (i + 1 < len) buf[i + 1] = (unsigned char)(x >> 16);
        if (i + 2 < len) buf[i + 2] = (unsigned char)(x >> 8);
        if (i + 3 < len) buf[i + 3] = (unsigned char)x;
    }
}

static void *hammer_fn(void *arg)
{
    long id = (long)arg;
    char path[64];
    static unsigned char buf[64 * 1024];
    snprintf(path, sizeof(path), "/test/bf-corpus-%ld.dat", id);
    off_t sz = (off_t)CORPUS_MB << 20;
    off_t off = (off_t)id * 65536;
    int fd = open(path, O_RDONLY);
    if (fd < 0) return NULL;
    while (!stop_flag) {
        /* 64 KB rotating reads: round-robin the whole file with a stride
         * so every lap touches pages another hammer evicted. 64 KB per
         * op (not 4 KB) lengthens the section under the big lock — the
         * barge window that starves parked waiters scales with it. */
        if (pread(fd, buf, sizeof(buf), off) < 0) break;
        off += 131072 + 65536;
        if (off >= sz) off = (off % 4096) + ((off / sz) % 3) * 4096;
        hammer_ops++;
    }
    close(fd);
    return NULL;
}

static void *churn_fn(void *arg)
{
    (void)arg;
    char path[] = "/test/bf-churn.dat";
    unsigned char buf[4096];
    unsigned seed = 7;
    while (!stop_flag) {
        fill_pattern(buf, sizeof(buf), seed++);
        int fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0644);
        if (fd < 0) break;
        for (int i = 0; i < CHURN_KB; i += 4)
            if (write(fd, buf, sizeof(buf)) < 0) goto out;
        for (int i = 0; i < CHURN_KB; i += 4)
            if (pread(fd, buf, sizeof(buf), (off_t)i << 10) < 0) goto out;
out:
        close(fd);
        unlink(path);
        churn_ops++;
    }
    return NULL;
}

static void *faulter_fn(void *arg)
{
    (void)arg;
    char path[] = "/test/bf-mmap.dat";
    off_t sz = (off_t)CORPUS_MB << 20;
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        printf("BIGFAIR-MMAP-FAIL open errno=%d\n", errno);
        return NULL;
    }
    unsigned char *p = mmap(NULL, (size_t)sz, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED) {
        printf("BIGFAIR-MMAP-FAIL mmap errno=%d\n", errno);
        close(fd);
        return NULL;
    }
    volatile unsigned char sink = 0;
    size_t off = 0;
    while (!stop_flag) {
        sink += p[off];
        off += 4096 * 97;
        if (off >= (size_t)sz) off -= (size_t)sz;
        fault_ops++;
    }
    (void)sink;
    munmap(p, (size_t)sz);
    close(fd);
    return NULL;
}

static void *canary_fn(void *arg)
{
    (void)arg;
    char path[] = "/test/bf-canary.dat";
    unsigned char buf[4096];
    off_t sz = (off_t)CANARY_MB << 20;
    off_t off = 0;
    int fd = open(path, O_RDONLY);
    if (fd < 0) return NULL;
    while (!stop_flag) {
        struct timespec sl = { 0, CANARY_PERIOD_MS * 1000L * 1000L };
        nanosleep(&sl, NULL);
        double t0 = now_ms();
        if (pread(fd, buf, sizeof(buf), off) < 0) break;
        double dt = now_ms() - t0;
        unsigned long ms = (unsigned long)(dt + 0.5);
        canary_n++;
        if (ms > 500) canary_over500++;
        if (ms > 1000) canary_over1s++;
        if (ms > canary_max_ms) canary_max_ms = ms;
        off += 8192;
        if (off >= sz) off = 0;
    }
    close(fd);
    return NULL;
}

int main(int argc, char **argv)
{
    long secs = argc > 1 ? atol(argv[1]) : 60;
    bound_ms = argc > 2 ? (unsigned long)atol(argv[2]) : 2000;

    /* Corpus: use pre-injected files when present (see run/bigfair.py:
     * mass in-guest file creation trips an unrelated text-page corruption
     * family in the write path; the fairness gate only needs the READ
     * side). Fall back to creating them only when missing. */
    static unsigned char wbuf[1 << 20];
    for (long i = 0; i < NHAMMER; i++) {
        char path[64];
        snprintf(path, sizeof(path), "/test/bf-corpus-%ld.dat", i);
        int fd = open(path, O_RDONLY);
        if (fd >= 0) {
            struct stat st;
            if (fstat(fd, &st) == 0 && st.st_size >= (CORPUS_MB << 20)) {
                close(fd);
                continue; /* pre-injected corpus is good */
            }
            close(fd);
        }
        fill_pattern(wbuf, sizeof(wbuf), (unsigned)i + 1);
        fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd < 0) {
            printf("BIGFAIR-FAIL-SETUP open %s errno=%d\n", path, errno);
            return 9;
        }
        for (int mb = 0; mb < CORPUS_MB; mb++)
            if (write(fd, wbuf, sizeof(wbuf)) < 0) {
                printf("BIGFAIR-FAIL-SETUP write errno=%d\n", errno);
                return 9;
            }
        close(fd);
    }
    {
        const char *extra[] = { "/test/bf-canary.dat", "/test/bf-mmap.dat" };
        int mb_for[] = { CANARY_MB, CORPUS_MB };
        for (int e = 0; e < 2; e++) {
            int fd = open(extra[e], O_RDONLY);
            if (fd >= 0) {
                struct stat st;
                if (fstat(fd, &st) == 0 && st.st_size >= (mb_for[e] << 20)) {
                    close(fd);
                    continue;
                }
                close(fd);
            }
            fill_pattern(wbuf, sizeof(wbuf), (unsigned)(100 + e));
            fd = open(extra[e], O_WRONLY | O_CREAT | O_TRUNC, 0644);
            if (fd < 0) {
                printf("BIGFAIR-FAIL-SETUP open %s errno=%d\n", extra[e], errno);
                return 9;
            }
            for (int mb = 0; mb < mb_for[e]; mb++)
                if (write(fd, wbuf, sizeof(wbuf)) < 0) {
                    printf("BIGFAIR-FAIL-SETUP write errno=%d\n", errno);
                    return 9;
                }
            close(fd);
        }
    }

    printf("BIGFAIR-START secs=%ld bound_ms=%lu corpus=%dMB canary=%dMB\n",
           secs, bound_ms, NHAMMER * CORPUS_MB, CANARY_MB);
    fflush(stdout);

    pthread_t th[NHAMMER + 3];
    for (long i = 0; i < NHAMMER; i++)
        pthread_create(&th[i], NULL, hammer_fn, (void *)i);
    pthread_create(&th[NHAMMER], NULL, churn_fn, NULL);
    pthread_create(&th[NHAMMER + 1], NULL, faulter_fn, NULL);
    pthread_create(&th[NHAMMER + 2], NULL, canary_fn, NULL);

    for (long t = 5; t <= secs; t += 5) {
        struct timespec sl = { 5, 0 };
        nanosleep(&sl, NULL);
        printf("BIGFAIR-T %ld hammer=%lu churn=%lu fault=%lu canary_n=%lu "
               "canary_max=%lums >500ms=%lu >1s=%lu\n",
               t, hammer_ops, churn_ops, fault_ops, canary_n,
               canary_max_ms, canary_over500, canary_over1s);
        fflush(stdout);
    }
    stop_flag = 1;
    for (int i = 0; i < NHAMMER + 3; i++)
        pthread_join(th[i], NULL);

    int pass = canary_max_ms < bound_ms;
    printf("BIGFAIR-RESULT pass=%d canary_n=%lu canary_max_ms=%lu "
           "over500ms=%lu over1s=%lu hammer_ops=%lu churn_ops=%lu "
           "fault_ops=%lu\n",
           pass, canary_n, canary_max_ms, canary_over500, canary_over1s,
           hammer_ops, churn_ops, fault_ops);
    printf(pass ? "BIGFAIR-PASS\n" : "BIGFAIR-FAIL canary_max=%lums\n",
           canary_max_ms);
    fflush(stdout);

    /* Best effort cleanup — churn file already unlinked; corpus is
     * reusable across runs, leave it (image is a private copy). */
    return pass ? 0 : 1;
}
