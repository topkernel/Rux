/*
 * heapwatch.c — damage detector for the intermittent userspace heap/VMA
 * corruption seen on the OpenHarmony boots (heap-forensics worktree,
 * docs/development/heap-corruption-timeline.md).
 *
 * What it hunts (all observed kernel-side signatures, userspace view):
 *   S1  page CONTENT of a live RW mapping replaced (zeroed or foreign
 *       data) — the mallocng get_meta garbage / zeroed-binder-BC class.
 *   S2  a live RW page stops being writable (VMA access bits stripped to
 *       PRIVATE-only -> "Permission denied") — the ld.so GWP-table class.
 *   S3  a page of a live mapping loses its VMA entirely (write faults
 *       with no VMA) — the b89 thread-stack-top class.
 *
 * Shape: three watched regions with per-page XOR checksums (binder-like
 * 1M mmap, stack-like mmap, malloc'd block), plus a thread that is
 * spawned fresh each round (malloc/free churn like an FFRT worker) and a
 * periodic fork+exit (mimics the DFX processdump fork whose COW burst
 * precedes damage in every boot log).
 *
 * Any fault in a watched region: the SIGSEGV handler prints the fault
 * address + a /proc/self/maps snapshot and exits 42.  Any checksum
 * mismatch: prints region/page/old/new first qword and the same maps
 * snapshot, exits 43.  A clean run prints HEAPWATCH-PASS every
 * HEARTBEAT rounds.
 *
 * Build (riscv64 minimal rootfs):
 *   riscv64-linux-gnu-gcc -static -pthread -O2 -Wall \
 *       -o heapwatch heapwatch.c
 * Run as init=/heapwatch or from the shell; prints to stdout.
 */
#define _GNU_SOURCE 1
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef MREMAP_MAYMOVE
#define MREMAP_MAYMOVE 1
#endif

static void dump_maps(void);

#define PAGE         4096UL
#define BINDER_SIZE  (1UL << 20)   /* matches the 1M binder mmap of victims */
#define STACK_SIZE   (2UL << 20)
#define HEAP_SIZE    (256UL << 10)
#define HEARTBEAT    2000
#define FORK_EVERY   64            /* rounds between fork+exit churns */
#define MREMAP_EVERY 16            /* rounds between mremap moves */

/*
 * mremap churn: the b86/b87 PTE-OVERWRITE-LIVE prints carry ctx=4
 * (mremap copy), so the suspected trigger is mremap traffic landing on
 * busy VMAs.  Allocate a 3M block, dirty it, move it with MAYMOVE,
 * verify the data survived the move, drop it.
 */
static void mremap_churn(int round)
{
    size_t sz = 3UL << 20;
    uint8_t *a = mmap(NULL, sz, PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (a == MAP_FAILED)
        return;
    for (size_t off = 0; off < sz; off += PAGE) {
        uint64_t *w = (uint64_t *)(a + off);
        w[0] = 0x4d52454d41502043ULL ^ (off / PAGE);   /* "CMAP REM" */
        w[1] = (uint64_t)round;
    }
    void *b = mremap(a, sz, sz, MREMAP_MAYMOVE);
    if (b == MAP_FAILED) {
        munmap(a, sz);
        return;
    }
    for (size_t off = 0; off < sz; off += PAGE) {
        uint64_t *w = (uint64_t *)((uint8_t *)b + off);
        if (w[0] != (0x4d52454d41502043ULL ^ (off / PAGE))) {
            printf("HEAPWATCH-MREMAP round=%d moved=%p page=%zu "
                   "first-qword=%016llx (mremap lost data!)\n",
                   round, b, off / PAGE,
                   (unsigned long long)w[0]);
            dump_maps();
            _exit(43);
        }
    }
    munmap(b, sz);
}

struct region {
    const char *name;
    uint8_t *base;
    size_t size;
    uint64_t *sums;                /* per-page checksum */
};

static struct region regions[3];
static int maps_fd = -1;

static void open_maps(void)
{
    maps_fd = open("/proc/self/maps", O_RDONLY);
    if (maps_fd < 0) {
        mkdir("/proc", 0755);
        if (mount("proc", "/proc", "proc", 0, NULL) == 0)
            maps_fd = open("/proc/self/maps", O_RDONLY);
    }
}

/* Async-signal-safe-ish: write() only. Good enough for a probe. */
static void dump_maps(void)
{
    char buf[512];
    ssize_t n;
    if (maps_fd < 0) {
        if (write(1, "HEAPWATCH: (no /proc/self/maps)\n", 32)) {}
        return;
    }
    if (write(1, "HEAPWATCH: --- /proc/self/maps ---\n", 35)) {}
    lseek(maps_fd, 0, SEEK_SET);
    while ((n = read(maps_fd, buf, sizeof(buf))) > 0)
        if (write(1, buf, n)) {}
    if (write(1, "HEAPWATCH: --- end maps ---\n", 28)) {}
}

static void sig_handler(int sig, siginfo_t *si, void *ctx)
{
    char buf[160];
    (void)ctx;
    int len = snprintf(buf, sizeof(buf),
                       "HEAPWATCH-FAULT sig=%d addr=%p (S2 flags-stripped or S3 vma-gone; "
                       "see maps below)\n", sig, si->si_addr);
    if (write(1, buf, len) < 0) {}
    dump_maps();
    _exit(42);
}

static uint64_t page_sum(const uint8_t *p)
{
    uint64_t s = 0;
    const uint64_t *w = (const uint64_t *)p;
    for (int i = 0; i < (int)(PAGE / 8); i++)
        s ^= w[i] * 0x100000001b3ULL + i;
    return s;
}

static void fill_region(struct region *r)
{
    /* recognizable pattern: page index in every 16-byte slot */
    for (size_t off = 0; off < r->size; off += 16) {
        uint64_t *slot = (uint64_t *)(r->base + off);
        size_t page = off / PAGE;
        slot[0] = 0x4845415057415443ULL ^ page;   /* "HEAPWATC" ^ page */
        slot[1] = 0x524547494f4e2140ULL + off;
    }
    for (size_t p = 0; p < r->size / PAGE; p++)
        r->sums[p] = page_sum(r->base + p * PAGE);
}

static void check_regions(int round)
{
    for (int i = 0; i < 3; i++) {
        struct region *r = &regions[i];
        for (size_t p = 0; p < r->size / PAGE; p++) {
            uint64_t now = page_sum(r->base + p * PAGE);
            if (now != r->sums[p]) {
                printf("HEAPWATCH-DAMAGE round=%d region=%s page=%zu "
                       "addr=%p oldsum=%016llx newsum=%016llx "
                       "first-qword=%016llx (S1 content replaced)\n",
                       round, r->name, p, r->base + p * PAGE,
                       (unsigned long long)r->sums[p],
                       (unsigned long long)now,
                       (unsigned long long)*(uint64_t *)(r->base + p * PAGE));
                dump_maps();
                _exit(43);
            }
        }
    }
}

/* Fresh worker: malloc/free churn + touch own stack top page, mimicking
 * an FFRT/IPC worker's first seconds of life (the observed victims). */
static void *fresh_worker(void *arg)
{
    (void)arg;
    for (int i = 0; i < 64; i++) {
        size_t sz = 32 + (i % 16) * 353;
        void *p = malloc(sz);
        if (!p)
            break;
        memset(p, (int)(i & 0xff), sz);
        free(p);
    }
    /* touch one byte near the top of our own stack (TCB side) */
    {
        char probe;
        char *approx_top = &probe;
        approx_top = (char *)(((uintptr_t)approx_top | (PAGE - 1)) + 1);
        approx_top[-1] ^= 0;   /* within current page: safe */
    }
    return NULL;
}

int main(void)
{
    struct sigaction sa;
    setvbuf(stdout, NULL, _IONBF, 0);
    open_maps();

    memset(&sa, 0, sizeof(sa));
    sa.sa_sigaction = sig_handler;
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGSEGV, &sa, NULL);
    sigaction(SIGBUS, &sa, NULL);

    regions[0].name = "binder-like-mmap";
    regions[0].base = mmap(NULL, BINDER_SIZE, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    regions[1].name = "stack-like-mmap";
    regions[1].base = mmap(NULL, STACK_SIZE, PROT_READ | PROT_WRITE,
                           MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    regions[2].name = "malloc-block";
    regions[2].base = malloc(HEAP_SIZE);
    regions[2].size = 0; /* set below */
    for (int i = 0; i < 3; i++) {
        if (i < 2)
            regions[i].size = (i == 0) ? BINDER_SIZE : STACK_SIZE;
        else
            regions[i].size = HEAP_SIZE;
        if (!regions[i].base || regions[i].base == MAP_FAILED) {
            printf("HEAPWATCH-FAIL alloc region %d\n", i);
            return 1;
        }
        regions[i].sums = calloc(regions[i].size / PAGE, sizeof(uint64_t));
        fill_region(&regions[i]);
    }
    /* make sure the malloc heap region is warmed through mallocng */
    for (int i = 0; i < 128; i++) {
        void *p = malloc(64 + i * 97);
        memset(p, 1, 64 + i * 97);
    }

    printf("HEAPWATCH-START binder=%p stack=%p heap=%p\n",
           regions[0].base, regions[1].base, regions[2].base);

    for (int round = 0;; round++) {
        pthread_t th;
        pthread_create(&th, NULL, fresh_worker, NULL);
        pthread_join(th, NULL);

        check_regions(round);

        /* S2/S3 sweep: rewrite same value on each page boundary —
         * faults (and reports) if access bits or the VMA vanished. */
        for (int i = 0; i < 3; i++) {
            volatile uint8_t *b = regions[i].base;
            for (size_t p = 0; p < regions[i].size / PAGE; p++)
                b[p * PAGE] = b[p * PAGE];
        }

        if (round % FORK_EVERY == FORK_EVERY - 1) {
            pid_t pid = fork();
            if (pid == 0)
                _exit(0);
            if (pid > 0)
                waitpid(pid, NULL, 0);
            check_regions(round);   /* post-fork COW-burst window */
        }

        if (round % MREMAP_EVERY == MREMAP_EVERY - 1) {
            mremap_churn(round);
            check_regions(round);   /* post-mremap window */
        }

        if (round % HEARTBEAT == 0) {
            printf("HEAPWATCH-BEAT round=%d\n", round);
            check_regions(round);
        }
    }
}
