// writestorm.c — fs/mm write-storm wedge repro (OH /data first-boot storm).
//
// Reproduces the oh-b120/b121 wedge family without OpenHarmony:
//   W1. page_cache::insert() self-deadlock: a reader streaming a large
//       file (page-cache inserts) while the zone is below watermark
//       (GFP_KERNEL slowpath -> direct reclaim -> page_cache::shrink ->
//       inodes.lock re-acquire). Old kernel: both CPUs spin on
//       PAGE_CACHE ("DEADLOCK: spinlock stuck ... page_cache::PAGE_CACHE")
//       with the ext4 big-lock holder wedged inside the insert. Fixed
//       kernel: survives.
//   W2. ext4 dir-entry storm: mkdir/create/unlink churn on ext4 while
//       the above pressure runs — the OH /data first-boot pattern
//       (add_dir_entry scan + sync I/O under the ext4 big lock).
//
// Topology (all inside one process running as init):
//   - memhog CHILD process: touch anon pages until ~75% of MemTotal,
//     hold, release, repeat (keeps the zone below WMARK_LOW so page
//     allocations take the slowpath; OOM-killing the child is fine).
//   - reader thread: pread() a 48 MB file round-robin (page-cache churn).
//   - fsstorm thread: mkdir/rmdir + create/unlink + rename churn.
//   - main: progress line every 5 s; WRITESTORM-PASS after <secs>.
//
// Verdict:
//   WRITESTORM-PASS            -> ok
//   WRITESTORM-FAIL-*          -> probe-internal failure
//   kernel DEADLOCK/soft lockup -> the wedge (driver reports rc 2)
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sched.h>
#include <time.h>
#include <pthread.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#define BIGFILE "big.dat"
#define BIGFILE_SZ (48UL << 20)
#define STORMDIR "storm"


static volatile sig_atomic_t stop_flag;
static unsigned long reader_pages, storm_ops;
static unsigned long memtotal_kb;

static void pfail(const char *what)
{
    char buf[128];
    int n = snprintf(buf, sizeof(buf), "WRITESTORM-FAIL-%s errno=%d\n", what, errno);
    if (n > 0)
        (void)write(2, buf, n);
    _exit(9);
}

static unsigned long read_memtotal_kb(void)
{
    int fd = open("/proc/meminfo", O_RDONLY);
    if (fd < 0)
        return 400000; /* guess for 512M guest */
    char buf[2048];
    int n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0)
        return 400000;
    buf[n] = 0;
    unsigned long kb = 0;
    if (sscanf(buf, "MemTotal: %lu kB", &kb) != 1)
        return 400000;
    return kb;
}

/* ---------------- memhog child ---------------- */
static int memhog_child(void)
{
    for (;;) {
        unsigned long target = memtotal_kb * 96 / 100; /* leave ~4% free: zone below WMARK_LOW */
        unsigned long chunk = 4UL << 20;
        unsigned long touched = 0;
        while (touched < target) {
            char *p = malloc(chunk);
            if (!p)
                break;
            for (unsigned long o = 0; o < chunk; o += 4096)
                p[o] = (char)(o ^ touched);
            touched += chunk;
        }
        sleep(25);
        _exit(0); /* parent respawns: alloc-hold-free pressure cycles */
    }
    return 0; /* unreachable */
}

/* ---------------- reader thread ---------------- */
struct thr_arg { long id; };

static void reader_thread(void *arg)
{
    (void)arg;
    char page[4096] __attribute__((aligned(4096)));
    int fd = open(BIGFILE, O_RDONLY);
    if (fd < 0)
        pfail("open-bigfile");
    off_t off = 0;
    while (!stop_flag) {
        ssize_t r = pread(fd, page, sizeof(page), off);
        if (r <= 0) {
            off = 0;
            continue;
        }
        off += r;
        __sync_fetch_and_add(&reader_pages, 1);
    }
}

/* ---------------- fsstorm thread ---------------- */
static void fsstorm_thread(void *arg)
{
    (void)arg;
    char path[256], path2[256];
    int i = 0;
    while (!stop_flag) {
        /* directory churn: mkdir N files inside, then remove */
        snprintf(path, sizeof(path), "%s/d%06d", STORMDIR, i % 4096);
        if (mkdir(path, 0755) == 0 || errno == EEXIST) {
            for (int k = 0; k < 8; k++) {
                char sub[320];
                snprintf(sub, sizeof(sub), "%s/f%d", path, k);
                int fd = open(sub, O_CREAT | O_RDWR | O_TRUNC, 0644);
                if (fd >= 0) {
                    (void)write(fd, sub, strlen(sub));
                    close(fd);
                }
                __sync_fetch_and_add(&storm_ops, 1);
            }
            /* rename one, unlink the rest, rmdir */
            snprintf(path2, sizeof(path2), "%s/d%06d-r", STORMDIR, i % 4096);
            rename(path, path2);
            for (int k = 0; k < 8; k++) {
                char sub[320];
                snprintf(sub, sizeof(sub), "%s/f%d", path2, k);
                unlink(sub);
            }
            rmdir(path2);
        }
        i++;
    }
}

static void make_bigfile(void)
{
    int fd = open(BIGFILE, O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0)
        pfail("create-bigfile");
    char page[4096];
    for (unsigned long o = 0; o < sizeof(page); o++)
        page[o] = (char)(o * 131);
    for (unsigned long off = 0; off < BIGFILE_SZ; off += sizeof(page)) {
        page[0] = (char)(off >> 12); /* vary content so pages are not dedupable */
        if (write(fd, page, sizeof(page)) != (ssize_t)sizeof(page))
            pfail("write-bigfile");
    }
    close(fd);
}

int main(int argc, char **argv)
{
    #ifdef PROBE_SECS
    int secs = PROBE_SECS;
#else
    int secs = 30;
#endif
    if (argc > 1)
        secs = atoi(argv[1]);

    setvbuf(stdout, NULL, _IONBF, 0);
    memtotal_kb = read_memtotal_kb();
    printf("writestorm: secs=%d memtotal=%lukB\n", secs, memtotal_kb);

    if (mkdir(STORMDIR, 0755) != 0 && errno != EEXIST)
        pfail("mkdir-stormdir");
    make_bigfile();
    printf("writestorm: bigfile ready (%lu MB)\n", BIGFILE_SZ >> 20);

    /* threads share the mm with init — same exposure as OH services */
    pthread_t t1, t2;
    if (pthread_create(&t1, NULL, (void *(*)(void *))reader_thread, NULL) != 0)
        pfail("pthread-reader");
    if (pthread_create(&t2, NULL, (void *(*)(void *))fsstorm_thread, NULL) != 0)
        pfail("pthread-storm");

    /* timed loop: keep a memhog child alive throughout, print progress */
    pid_t hog = -1;
    for (int s = 0; s < secs; s += 5) {
        int st;
        while (waitpid(-1, &st, WNOHANG) > 0) { /* reap */ }
        hog = fork();
        if (hog == 0)
            _exit(memhog_child());
        if (hog < 0)
            pfail("fork-memhog");
        struct timespec ts = {5, 0};
        nanosleep(&ts, NULL);
        printf("writestorm: t+%ds reader_pages=%lu storm_ops=%lu\n",
               s + 5, reader_pages, storm_ops);
    }
    stop_flag = 1;
    struct timespec ts = {1, 0};
    nanosleep(&ts, NULL);
    printf("WRITESTORM-PASS reader_pages=%lu storm_ops=%lu\n", reader_pages, storm_ops);
    sync();
    return 0;
}
