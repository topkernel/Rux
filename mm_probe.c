/* mm_probe.c — GB-workload-shaped memory cost probe.
 *
 * Times the four patterns Geekbench's allocating workloads pay, as
 * ns-per-operation, on whichever kernel boots the image:
 *   A. anon sequential alloc+touch+free (graph construction shape)
 *   B. anon alloc + RANDOM touch (pointer-chasing data structure shape)
 *   C. file-backed mmap random touch (lat_pagefault shape, 16M file)
 *   D. mmap/munmap only, no touch (VMA + TLB-shootdown path alone)
 *   E. brk growth (small_malloc arena growth shape)
 *   F. fork+exit (thread create/join shape — clone+futex round trip)
 *
 * Prints one result line per pattern. PID 1; writes to /dev/console.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <pthread.h>
#include <time.h>

#define MB (1024UL * 1024UL)
#define ROUNDS 8

static double now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1e9 + ts.tv_nsec;
}

static int rnd_state = 12345;
static unsigned rnd(void) {
    rnd_state = rnd_state * 1103515245 + 12345;
    return (unsigned)(rnd_state >> 8);
}

static void hdr(const char *name) { printf("MM-PROBE %s\n", name); fflush(stdout); }

/* worker for section H/I */
static pthread_mutex_t fm = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t fc = PTHREAD_COND_INITIALIZER;
static volatile int fcount = 0;
static pthread_mutex_t fm2 = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t fc2 = PTHREAD_COND_INITIALIZER;
static volatile int fdone = 0;

static void *spinner(void *arg) {
    (void)arg;
    pthread_mutex_lock(&fm);
    while (fcount == 0)
        pthread_cond_wait(&fc, &fm);
    fcount = 0;
    pthread_mutex_unlock(&fm);
    return NULL;
}

/* section I: ping-pong partner, exits after the target round count */
static volatile long ftarget = 1000;
static void *ponger(void *arg) {
    (void)arg;
    for (;;) {
        pthread_mutex_lock(&fm);
        while (fcount == 0)
            pthread_cond_wait(&fc, &fm);
        fcount = 0;
        pthread_mutex_unlock(&fm);
        pthread_mutex_lock(&fm2);
        fdone += 1;
        pthread_cond_signal(&fc2);
        pthread_mutex_unlock(&fm2);
        if (fdone >= ftarget) return NULL;
    }
    return NULL;
}

int main(void) {
    int console = open("/dev/console", O_WRONLY);
    if (console < 0) console = 1;
    dup2(console, 1); dup2(console, 2);
    /* C needs /tmp: mount tmpfs if not mounted */
    mkdir("/tmp", 0755);
    if (mount("none", "/tmp", "tmpfs", 0, NULL) != 0 && errno != EBUSY)
        printf("MM-PROBE tmpfs mount rc=%d\n", -errno);

    const unsigned long PAGES = 64UL * MB / 4096UL;   /* 16384 pages */
    char *perm = malloc(PAGES * sizeof(unsigned));
    for (unsigned long i = 0; i < PAGES; i++) perm[i] = (char)(i & 0xff);

    /* ---- A. anon sequential alloc+touch+free ---- */
    {
        hdr("A anon-seq 64MB write-touch, per-fault ns");
        double best = 1e18;
        for (int r = 0; r < ROUNDS; r++) {
            double t0 = now_ns();
            char *p = mmap(NULL, 64 * MB, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
            if (p == MAP_FAILED) { printf("A mmap failed\n"); break; }
            for (unsigned long i = 0; i < PAGES; i++) p[i * 4096] = (char)i;
            munmap(p, 64 * MB);
            double dt = now_ns() - t0;
            if (dt < best) best = dt;
        }
        printf("MM-RESULT A per_fault_ns=%.1f round_ms=%.1f\n",
               best / (double)PAGES, best / 1e6);
        fflush(stdout);
    }

    /* ---- B. anon random touch ---- */
    {
        hdr("B anon-rand 64MB random write-touch, per-fault ns");
        double best = 1e18;
        for (int r = 0; r < ROUNDS; r++) {
            double t0 = now_ns();
            char *p = mmap(NULL, 64 * MB, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
            if (p == MAP_FAILED) { printf("B mmap failed\n"); break; }
            for (unsigned long k = 0; k < PAGES; k++) {
                unsigned long i = (unsigned long)(rnd() % PAGES);
                p[i * 4096] = (char)k;
            }
            munmap(p, 64 * MB);
            double dt = now_ns() - t0;
            if (dt < best) best = dt;
        }
        printf("MM-RESULT B per_fault_ns=%.1f round_ms=%.1f\n",
               best / (double)PAGES, best / 1e6);
        fflush(stdout);
    }

    /* ---- C. file-backed random touch (lat_pagefault shape) ---- */
    {
        hdr("C file-mmap 16MB random read-touch, per-fault ns");
        const char *fn = "/tmp/mmprobe.file";
        int fd = open(fn, O_RDWR | O_CREAT | O_TRUNC, 0600);
        if (fd >= 0) {
            char buf[4096];
            memset(buf, 7, sizeof(buf));
            for (int i = 0; i < 4096; i++) write(fd, buf, 4096); /* 16MB */
            fsync(fd);
            double best = 1e18;
            for (int r = 0; r < ROUNDS; r++) {
                double t0 = now_ns();
                char *p = mmap(NULL, 16 * MB, PROT_READ, MAP_SHARED, fd, 0);
                if (p == MAP_FAILED) { printf("C mmap failed\n"); break; }
                volatile int sum = 0;
                for (unsigned long k = 0; k < 4096; k++) {
                    unsigned long i = (unsigned long)(rnd() % 4096);
                    sum += p[i * 4096];
                }
                munmap(p, 16 * MB);
                double dt = now_ns() - t0;
                if (dt < best) best = dt;
            }
            printf("MM-RESULT C per_fault_ns=%.1f round_ms=%.1f\n",
                   best / 4096.0, best / 1e6);
            close(fd);
            unlink(fn);
        } else {
            printf("MM-RESULT C skipped (no /tmp)\n");
        }
        fflush(stdout);
    }

    /* ---- D. mmap/munmap only, split timing (min + mean) ---- */
    {
        hdr("D mmap-munmap 64MB no touch, split per-op ns");
        double bmap = 1e18, bunmap = 1e18, smap = 0, sunmap = 0;
        int n = ROUNDS * 4;
        for (int r = 0; r < n; r++) {
            double t0 = now_ns();
            char *p = mmap(NULL, 64 * MB, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
            if (p == MAP_FAILED) { printf("D mmap FAILED errno=%d\n", errno); break; }
            double t1 = now_ns();
            munmap(p, 64 * MB);
            double t2 = now_ns();
            if (t1 - t0 < bmap) bmap = t1 - t0;
            if (t2 - t1 < bunmap) bunmap = t2 - t1;
            smap += t1 - t0;
            sunmap += t2 - t1;
        }
        printf("MM-RESULT D mmap_ns=%.1f mean=%.1f munmap_ns=%.1f mean=%.1f\n",
               bmap, smap / n, bunmap, sunmap / n);
        fflush(stdout);
    }

    /* ---- D2. sizes sweep: munmap of 2MB and 8MB ---- */
    {
        hdr("D2 munmap size sweep, per-op ns");
        for (unsigned long mb = 2; mb <= 64; mb *= 8) {
            double bunmap = 1e18;
            for (int r = 0; r < ROUNDS * 2; r++) {
                char *p = mmap(NULL, mb * MB, PROT_READ | PROT_WRITE,
                               MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
                double t0 = now_ns();
                munmap(p, mb * MB);
                double dt = now_ns() - t0;
                if (dt < bunmap) bunmap = dt;
            }
            printf("MM-RESULT D2 munmap_%luMB_ns=%.1f\n", mb, bunmap);
            fflush(stdout);
        }
    }

    /* ---- D3. mprotect growth pattern (glibc thread-arena shape) ---- */
    {
        hdr("D3 mprotect arena growth: PROT_NONE 64MB then 16x mprotect-RW 4MB, per-op ns");
        double bmprot = 1e18;
        for (int r = 0; r < ROUNDS; r++) {
            char *p = mmap(NULL, 64 * MB, PROT_NONE,
                           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
            double t0 = now_ns();
            for (int i = 0; i < 16; i++)
                mprotect(p + i * 4 * MB, 4 * MB, PROT_READ | PROT_WRITE);
            double dt = now_ns() - t0;
            if (dt < bmprot) bmprot = dt;
            munmap(p, 64 * MB);
        }
        printf("MM-RESULT D3 mprotect_per_op_ns=%.1f\n", bmprot / 16.0);
        fflush(stdout);
    }

    /* ---- E. brk growth ---- */
    {
        hdr("E brk 4096x4KB growth, per-call ns");
        double best = 1e18;
        for (int r = 0; r < ROUNDS; r++) {
            double t0 = now_ns();
            for (int i = 0; i < 4096; i++) {
                void *p = sbrk(4096);
                if (p == (void *)-1) { printf("E sbrk failed\n"); break; }
                ((char *)p)[0] = 1;   /* touch the new page tail */
                ((char *)p)[4095] = 2;
            }
            double dt = now_ns() - t0;
            if (dt < best) best = dt;
        }
        printf("MM-RESULT E per_call_ns=%.1f\n", best / 4096.0);
        fflush(stdout);
    }

    /* ---- F. fork+exit (thread-ish handoff shape) ---- */
    {
        hdr("F fork-exit x256, per-op ns");
        double best = 1e18;
        for (int r = 0; r < 4; r++) {
            double t0 = now_ns();
            for (int i = 0; i < 256; i++) {
                pid_t pid = fork();
                if (pid == 0) _exit(0);
                waitpid(pid, NULL, 0);
            }
            double dt = now_ns() - t0;
            if (dt < best) best = dt;
        }
        printf("MM-RESULT F per_op_ns=%.1f\n", best / 256.0);
        fflush(stdout);
    }

    /* ---- G. glibc malloc/free churn (the GB allocator substrate) ---- */
    {
        hdr("G glibc malloc-touch-free churn");
        for (unsigned long mb = 1; mb <= 16; mb *= 16) {
            size_t sz = mb * MB;
            int n = (int)(64 * MB / sz);
            if (n > 256) n = 256;
            double best = 1e18;
            for (int r = 0; r < ROUNDS; r++) {
                double t0 = now_ns();
                for (int i = 0; i < n; i++) {
                    char *p = malloc(sz);
                    p[0] = 1;
                    p[sz - 1] = 2;
                    free(p);
                }
                double dt = now_ns() - t0;
                if (dt < best) best = dt;
            }
            printf("MM-RESULT G malloc_%luMB per_op_ns=%.1f (n=%d)\n", mb, best / n, n);
            fflush(stdout);
        }
    }

    /* ---- H. pthread create+join (MC thread-row shape) ---- */
    {
        hdr("H pthread create-join x128, per-op ns");
        double best = 1e18;
        for (int r = 0; r < 4; r++) {
            double t0 = now_ns();
            for (int i = 0; i < 128; i++) {
                pthread_t th;
                fcount = 1;   /* let the fresh worker exit immediately */
                pthread_create(&th, NULL, spinner, NULL);
                pthread_join(th, NULL);
            }
            double dt = now_ns() - t0;
            if (dt < best) best = dt;
        }
        printf("MM-RESULT H per_op_ns=%.1f\n", best / 128.0);
        fflush(stdout);
    }

    /* ---- I. futex handoff round trip (thread-pool iteration shape) ---- */
    {
        hdr("I futex ping-pong x1000 (2 wakes per round), per-op ns");
        pthread_t th;
        pthread_create(&th, NULL, ponger, NULL);
        double best = 1e18;
        for (int r = 0; r < 4; r++) {
            double t0 = now_ns();
            for (int i = 0; i < 1000; i++) {
                if ((i % 100) == 0) { printf("I round=%d\n", i); fflush(stdout); }
                pthread_mutex_lock(&fm);
                fcount = 1;                 /* set the predicate, THEN signal */
                pthread_cond_signal(&fc);   /* kick the worker */
                pthread_mutex_unlock(&fm);
                pthread_mutex_lock(&fm2);
                while (!fdone)
                    pthread_cond_wait(&fc2, &fm2);
                fdone = 0;
                pthread_mutex_unlock(&fm2);
            }
            double dt = now_ns() - t0;
            if (dt < best) best = dt;
        }
        pthread_join(th, NULL);
        printf("MM-RESULT I per_op_ns=%.1f\n", best / 1000.0);
        fflush(stdout);
    }

    printf("MM-PROBE done\n");
    fflush(stdout);
    sync();
    _exit(0);
}
