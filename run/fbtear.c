/* fbtear — does concurrent munmap churn tear the fbdev device mapping?
 *
 * Xorg's exact shape (the black-screen hypothesis): one thread holds a
 * writable MAP_SHARED mapping of /dev/fb0 and memcpys into it (the
 * shadow->fb refresh), while sibling threads of the SAME process churn
 * mmap/munmap (pixmaps, GC buffers, module loads). If the kernel's
 * munmap leaf-table walk mis-tears the fb mapping's PTEs, the writer's
 * stores land on fresh anonymous pages: the writer succeeds, Xorg
 * stays alive, and the PHYSICAL framebuffer stays zero — the observed
 * black screen.
 *
 * Detector: two independent mappings of the same fb (alias at a second
 * address via MAP_FIXED). Writer writes a rotating pattern through
 * map1; verifier reads through map2 (same physical pages). If map1 was
 * redirected, map2 reads zeros/stale while map1's own read-back still
 * matches (anonymous pages are private).
 *
 * Exit code = number of failures.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define W 1280
#define H 800
#define FB_LEN (W * H * 4)
#define ALIAS 0x600000000000ULL /* map2 address, far from the churn area */
#define CHURN_THREADS 4

static unsigned char *map1;
static volatile int stop;
static volatile long torn_events;
static volatile long verif_misses;

static void *churn(void *arg)
{
    unsigned seed = (unsigned)(uintptr_t)arg * 2654435761u;
    while (!stop) {
        seed = seed * 1103515245 + 12345;
        size_t sz = 0x40000 + (seed % 0x20) * 0x40000; /* 256KB..8MB */
        void *m = mmap(NULL, sz, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (m == MAP_FAILED) continue;
        memset(m, (int)(seed & 0xff), 4096);
        munmap(m, sz);
    }
    return NULL;
}

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    int fd = open("/dev/fb0", O_RDWR);
    if (fd < 0) { printf("FBTEAR open failed\n"); return 9; }

    map1 = mmap(NULL, FB_LEN, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    unsigned char *map2 = mmap((void *)ALIAS, FB_LEN, PROT_READ | PROT_WRITE,
                               MAP_SHARED | MAP_FIXED, fd, 0);
    if (map1 == MAP_FAILED || map2 == MAP_FAILED) {
        printf("FBTEAR mmap failed (%s)\n", strerror(errno));
        return 9;
    }

    pthread_t th[CHURN_THREADS];
    for (int i = 0; i < CHURN_THREADS; i++)
        pthread_create(&th[i], NULL, churn, (void *)(uintptr_t)(i + 1));

    unsigned char pat = 1;
    for (int round = 0; round < 20000 && !stop; round++) {
        /* Writer: paint map1's check row with the rotating pattern. */
        unsigned char *row = map1 + (H / 2) * (W * 4);
        memset(row, pat, W * 4);
        /* Memory barrier: force the stores out before the alias read. */
        __sync_synchronize();
        /* Verifier: the SAME physical pages through map2. */
        unsigned char *arow = map2 + (H / 2) * (W * 4);
        int miss = 0;
        for (int i = 0; i < W * 4; i += 997) {
            if (arow[i] != pat) { miss = 1; break; }
        }
        if (miss) {
            /* map1's own view: if map1 still sees the pattern but map2
             * does not, map1's stores landed on PRIVATE pages = torn. */
            __sync_synchronize();
            int self_ok = 1;
            for (int i = 0; i < W * 4; i += 997)
                if (row[i] != pat) { self_ok = 0; break; }
            if (self_ok) {
                torn_events++;
                if (torn_events <= 5)
                    printf("FBTEAR TORN round=%d pat=%02x — writer sees pattern, alias does not: mapping redirected\n",
                           round, pat);
            } else {
                verif_misses++; /* both views wrong: pattern vanished entirely */
            }
        }
        pat = (unsigned char)(pat + 7);
        if (round % 5000 == 0) {
            printf("FBTEAR progress %d torn=%ld miss=%ld\n",
                   round, torn_events, verif_misses);
            fflush(stdout);
        }
    }
    stop = 1;
    for (int i = 0; i < CHURN_THREADS; i++) pthread_join(th[i], NULL);

    printf("FBTEAR-DONE torn=%ld vanished=%ld\n", torn_events, verif_misses);
    return (torn_events + verif_misses) > 0 ? 1 : 0;
}
