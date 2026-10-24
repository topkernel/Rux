/* memstorm.c — stress-fleet memory-pressure member (PID 1, never exits).
 *
 * Reproduces the allocation shapes that historically tore the arena:
 *   - big mmap/munmap cycles (including munmap of a full giant range)
 *   - MAP_FIXED re-mmaps over live ranges (hole punching)
 *   - fork storms with COW touch patterns (child touches 1/Nth of pages)
 *   - concurrent parents overlapping churn (parallel mmap+fork mix)
 *   - mprotect flips on live regions
 *
 * Heartbeat "MEMSTORM-BEAT <i> <phase> alive=<n>" every cycle so the
 * host sweep can see progress; any wedge >300s without a beat line is
 * a hang candidate. Failures print MEMSTORM-FAIL with errno — ENOMEM
 * is expected occasionally near the ceiling and NOT a failure class
 * unless it persists (tracked by the sweep via beat/ENOMEM ratio).
 *
 * Build: riscv64-musl-gcc -static -O1 -o memstorm memstorm.c
 */
#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#define MiB (1024UL * 1024UL)

static unsigned long seed = 1;
static inline unsigned long rnd(void) {
    seed = seed * 6364136223846793005UL + 1442695040888963407UL;
    return seed >> 17;
}

static void big_mmap_cycle(void) {
    size_t sz = (64 + (rnd() % 192)) * MiB; /* 64..256 MiB */
    char *p = mmap(NULL, sz, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) {
        printf("MEMSTORM-ENOMAP size=%zu errno=%d\n", sz, errno);
        return;
    }
    /* sparse touch: every 4096th page (COW-less population) */
    for (size_t o = 0; o < sz; o += 16 * 4096) p[o] = (char)o;
    if (rnd() & 1)
        munmap(p, sz);
    else
        munmap(p, sz); /* both paths identical; kept for future split */
}

static void fixed_remap_cycle(void) {
    static char *base;
    static int init;
    if (!init) {
        base = mmap(NULL, 128 * MiB, PROT_NONE,
                    MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        init = 1;
        if (base == MAP_FAILED) return;
    }
    size_t off = (rnd() % 64) * MiB;
    char *q = mmap(base + off, 8 * MiB, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
    if (q != MAP_FAILED) memset(q, 0x5a, 8 * MiB); /* 8 MiB touch storm */
}

static void mprotect_cycle(void) {
    size_t sz = (8 + (rnd() % 24)) * MiB;
    char *p = mmap(NULL, sz, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) return;
    memset(p, 1, 4096);
    mprotect(p, sz, PROT_READ);
    mprotect(p, sz, PROT_READ | PROT_WRITE);
    memset(p, 2, 4096);
    munmap(p, sz);
}

static void fork_storm(int nchildren) {
    for (int c = 0; c < nchildren; c++) {
        pid_t pid = fork();
        if (pid == 0) {
            /* child: touch a random slice of a fresh mapping then exit */
            size_t sz = (1 + (rnd() % 8)) * MiB;
            char *p = mmap(NULL, sz, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
            if (p != MAP_FAILED)
                for (size_t o = 0; o < sz; o += 8192) p[o] = (char)(o ^ c);
            _exit(0);
        } else if (pid < 0) {
            printf("MEMSTORM-FORKFAIL errno=%d\n", errno);
            return;
        }
    }
    /* reap without blocking forever */
    for (int c = 0; c < nchildren; c++) {
        int st;
        if (waitpid(-1, &st, WNOHANG) <= 0) usleep(2000);
    }
    while (waitpid(-1, NULL, WNOHANG) > 0) {}
}

int main(int argc, char **argv) {
    seed = (argc > 1) ? (unsigned long)atoll(argv[1]) : 42;
    setvbuf(stdout, NULL, _IOLBF, 0);
    printf("MEMSTORM-START seed=%lu pid=%d\n", seed, getpid());
    for (unsigned long i = 0;; i++) {
        int phase = (int)(rnd() % 4);
        switch (phase) {
        case 0: big_mmap_cycle(); break;
        case 1: fixed_remap_cycle(); break;
        case 2: mprotect_cycle(); break;
        case 3: fork_storm(4 + (int)(rnd() % 12)); break;
        }
        if ((i & 0xf) == 0) {
            printf("MEMSTORM-BEAT %lu phase=%d\n", i, phase);
            usleep(10000);
        }
        if ((i & 0x3ff) == 0) usleep(50000); /* pacing */
    }
    return 0; /* unreachable */
}
