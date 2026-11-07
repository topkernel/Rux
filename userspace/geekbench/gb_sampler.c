/* gb_sampler.c — guest-side CPU-clock IP sampler for the GB flamegraph.
 *
 * The kernel's perf slice (kernel/src/perf) implements
 * PERF_TYPE_SOFTWARE / PERF_COUNT_SW_CPU_CLOCK with sample_period (ns
 * of counted time, tick-granularity coalescing) and PERF_SAMPLE_IP
 * records consumed via read(2). Per-CPU form: pid=-1, cpu=N.
 *
 * The sampler forks from geekbench_run before it execve's the suite,
 * opens one event per CPU (kernel+user samples, idle excluded), polls
 * the fds once per second into a flat IP histogram, and at deadline
 * prints "GBSAMP ip count" lines (top 2048 by count) plus mode buckets
 * to stdout. Symbolization happens on the host against the kernel ELF
 * (kernel-text IPs) — user-text IPs resolve against the GB binaries.
 *
 * Static musl build:
 *   musl-gcc -static -O2 -o gb_sampler gb_sampler.c
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <time.h>
#include <sys/syscall.h>

#ifndef __NR_perf_event_open
#define __NR_perf_event_open 291 /* x86_64 */
#endif

#define PERF_TYPE_SOFTWARE 1
#define PERF_COUNT_SW_CPU_CLOCK 0
#define PERF_SAMPLE_IP (1UL << 0)
#define FLAG_EXCLUDE_IDLE (1UL << 7)

struct perf_event_attr_min {
    unsigned int type;
    unsigned int size;
    unsigned long long config;
    unsigned long long sample_period;
    unsigned long long sample_type;
    unsigned long long read_format;
    unsigned long long flags1;
};

#define MAX_CPUS 8
#define HASH_BITS 14
#define HASH_SZ (1 << HASH_BITS)

struct ent {
    unsigned long long ip;
    unsigned long long n;
    int used;
};
static struct ent htab[HASH_SZ];

static unsigned long long kern_n, user_n, other_n;

static void htab_add(unsigned long long ip, unsigned short misc)
{
    if ((misc & 0x7) == 2) kern_n++;        /* PERF_RECORD_MISC_KERNEL */
    else if ((misc & 0x7) == 1) user_n++;   /* PERF_RECORD_MISC_USER */
    else other_n++;
    unsigned int i = (unsigned int)((ip >> 4) * 0x9E3779B1u) & (HASH_SZ - 1);
    for (int k = 0; k < HASH_SZ; k++) {
        struct ent *e = &htab[i];
        if (!e->used) {
            e->used = 1;
            e->ip = ip;
            e->n = 1;
            return;
        }
        if (e->ip == ip) {
            e->n++;
            return;
        }
        i = (i + 1) & (HASH_SZ - 1);
    }
    /* table full: drop */
}

static int cmp_ent(const void *a, const void *b)
{
    const struct ent *x = a, *y = b;
    if (y->n > x->n) return 1;
    if (y->n < x->n) return -1;
    return 0;
}

int main(int argc, char **argv)
{
    long seconds = argc > 1 ? atol(argv[1]) : 90;
    int fds[MAX_CPUS];
    int ncpu = 0;

    for (int c = 0; c < MAX_CPUS; c++) {
        struct perf_event_attr_min attr;
        memset(&attr, 0, sizeof(attr));
        attr.type = PERF_TYPE_SOFTWARE;
        attr.size = sizeof(attr);
        attr.config = PERF_COUNT_SW_CPU_CLOCK;
        attr.sample_period = 10000000ULL; /* 10 ms = one tick */
        attr.sample_type = PERF_SAMPLE_IP;
        attr.flags1 = FLAG_EXCLUDE_IDLE;
        int fd = (int)syscall(__NR_perf_event_open, &attr, -1, c, -1, 0);
        if (fd < 0)
            break;
        fcntl(fd, F_SETFL, O_NONBLOCK);
        fds[ncpu++] = fd;
    }
    if (!ncpu) {
        printf("GBSAMP no-event errno=%d\n", errno);
        return 1;
    }
    printf("GBSAMP start cpus=%d secs=%ld\n", ncpu, seconds);

    struct timespec ts0;
    clock_gettime(CLOCK_MONOTONIC, &ts0);
    for (;;) {
        struct timespec d = { 1, 0 };
        nanosleep(&d, NULL);
        for (int i = 0; i < ncpu; i++) {
            static char buf[65536];
            for (;;) {
                ssize_t r = read(fds[i], buf, sizeof(buf));
                if (r <= 0)
                    break;
                for (ssize_t o = 0; o + 16 <= r; o += 16) {
                    unsigned int type = *(unsigned int *)(buf + o);
                    unsigned short misc = *(unsigned short *)(buf + o + 4);
                    unsigned long long ip = *(unsigned long long *)(buf + o + 8);
                    if (type == 9) /* PERF_RECORD_SAMPLE */
                        htab_add(ip, misc);
                }
                if (r < (ssize_t)sizeof(buf))
                    break;
            }
        }
        struct timespec now;
        clock_gettime(CLOCK_MONOTONIC, &now);
        if (now.tv_sec - ts0.tv_sec >= seconds)
            break;
    }

    static struct ent ents[HASH_SZ];
    int n = 0;
    for (int i = 0; i < HASH_SZ; i++)
        if (htab[i].used)
            ents[n++] = htab[i];
    qsort(ents, n, sizeof(ents[0]), cmp_ent);
    printf("GBSAMP end total_ips=%d kern=%llu user=%llu other=%llu\n",
           n, kern_n, user_n, other_n);
    for (int i = 0; i < n && i < 2048; i++)
        printf("GBSAMP %llx %llu\n", ents[i].ip, ents[i].n);
    printf("GBSAMP done\n");
    return 0;
}
