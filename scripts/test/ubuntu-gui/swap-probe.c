// swap-probe — exceed physical RAM with anonymous memory, relying on swap.
//
// Static gate probe for the swap activation (mm/swap.rs tail carve of the
// root disk). Strategy: mmap a 3 GiB anonymous window, touch it in 256 MiB
// increments until strictly more than 2 GiB has been faulted in, stopping
// early only when MemAvailable + SwapFree headroom runs low (PASS requires
// surviving — the OOM killer must never fire). Every touched page gets a
// per-page pattern; a 1/8-sample verify pass re-reads pages afterwards so
// pages the reclaim engine swapped out must fault back in through the
// swap-in path with their contents intact.
//
// Exit 0 + "swap-probe: PASS" on serial iff:
//   - swap was active (SwapTotal > 0),
//   - strictly more than 2 GiB was touched and the process survived,
//   - all sampled pages verify.
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define GiB (1024UL * 1024 * 1024)
#define CHUNK (256UL * 1024 * 1024) /* touch increment */
#define VA_SIZE (3UL * GiB)         /* VA window (overcommit heuristic) */
#define TARGET (2UL * GiB)          /* PASS threshold: touched > 2 GiB */
#define MARGIN (16UL * 1024 * 1024) /* stop touching below this headroom */
#define PAGE 4096UL

static unsigned long meminfo_kb(const char *key)
{
    int fd = open("/proc/meminfo", O_RDONLY);
    if (fd < 0)
        return 0;
    char buf[4096];
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0)
        return 0;
    buf[n] = 0;
    char pat[64];
    snprintf(pat, sizeof(pat), "%s:", key);
    char *p = strstr(buf, pat);
    if (!p)
        return 0;
    return strtoul(p + strlen(pat), NULL, 10); /* value is in kB */
}

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);

    unsigned long swap_total = meminfo_kb("SwapTotal");
    if (!swap_total) {
        printf("swap-probe: FAIL no swap active (SwapTotal=0)\n");
        return 1;
    }
    printf("swap-probe: SwapTotal=%lu kB SwapFree=%lu kB MemAvailable=%lu kB\n",
           swap_total, meminfo_kb("SwapFree"), meminfo_kb("MemAvailable"));

    char *base = mmap(NULL, VA_SIZE, PROT_READ | PROT_WRITE,
                      MAP_ANONYMOUS | MAP_PRIVATE, -1, 0);
    if (base == MAP_FAILED) {
        printf("swap-probe: FAIL mmap errno=%d\n", errno);
        return 1;
    }

    const unsigned long fine = 16UL * 1024 * 1024;   /* 16 MiB steps near the edge */
    const unsigned long fine_pages = fine / PAGE;
    unsigned long touched = 0;
    unsigned long swap_used_peak_kb = 0;

    /*
     * Touch loop. MemAvailable here equals buddy free pages, so while free
     * memory stays above the reclaim watermarks the kernel has no reason to
     * swap out — the probe must keep touching PAST the physical ceiling and
     * let direct reclaim spill the cold chunks to swap. Coarse 256 MiB
     * chunks while headroom is plentiful, 32 MiB steps near the edge so the
     * process stops before the OOM killer fires.
     */
    while (touched + fine <= VA_SIZE) {
        unsigned long headroom_kb =
            meminfo_kb("MemAvailable") + meminfo_kb("SwapFree");
        unsigned long step = (headroom_kb > 512 * 1024) ? CHUNK : fine;
        unsigned long step_pages = step / PAGE;
        if (headroom_kb * 1024 < step + MARGIN) {
            printf("swap-probe: headroom low (%lu kB), stopping\n",
                   headroom_kb);
            break;
        }

        char *chunk = base + touched;
        for (unsigned long i = 0; i < step_pages; i++) {
            unsigned long idx = touched / PAGE + i;
            unsigned long *w = (unsigned long *)(chunk + i * PAGE);
            w[0] = 0x53576170527830ULL ^ idx; /* 'SwapRx0' xor index */
            w[PAGE / 8 - 1] = idx;
        }
        touched += step;

        unsigned long used_kb = swap_total - meminfo_kb("SwapFree");
        if (used_kb > swap_used_peak_kb)
            swap_used_peak_kb = used_kb;
        if (step == CHUNK || (touched / fine) % 8 == 0 || used_kb > 0)
            printf("swap-probe: touched %lu MiB, MemAvail %lu kB, swap used %lu kB\n",
                   touched >> 20, meminfo_kb("MemAvailable"), used_kb);

        /* PASS needs strictly more than 2 GiB — stop as soon as proven. */
        if (touched > TARGET)
            break;
    }

    if (touched <= TARGET) {
        printf("swap-probe: FAIL only %lu MiB touched (need >2048)\n",
               touched >> 20);
        return 1;
    }

    /* Verify pass: 1/8 sample — swapped-out pages fault back in here. */
    const unsigned long total_pages = touched / PAGE;
    unsigned long sampled = 0, bad = 0;
    for (unsigned long idx = 0; idx < total_pages; idx += 8) {
        unsigned long *w = (unsigned long *)(base + idx * PAGE);
        sampled++;
        if (w[0] != (0x53576170527830ULL ^ idx) ||
            w[PAGE / 8 - 1] != idx)
            bad++;
    }

    printf("swap-probe: verify sampled=%lu bad=%lu, peak swap used %lu kB\n",
           sampled, bad, swap_used_peak_kb);

    if (bad) {
        printf("swap-probe: FAIL %lu/%lu pages corrupt\n", bad, sampled);
        return 1;
    }

    printf("swap-probe: PASS touched=%lu MiB (>2048), peak swap used %lu kB\n",
           touched >> 20, swap_used_peak_kb);
    return 0;
}
