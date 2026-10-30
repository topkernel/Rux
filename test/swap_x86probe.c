/* swap_x86probe.c — x86_64 swap-out end-to-end contract (FD-1).
 *
 * Before the per-arch leaf primitives, mm/rmap.rs try_to_unmap_inner
 * walked user page tables with Sv39 shifts (root indexed by bits 38:30)
 * — on the 4-level x86_64 tree that read the wrong PML4 slot, the walk
 * failed, and swap-out "unmapped" nothing: anonymous reclaim freed no
 * page and every swap slot was handed straight back.
 *
 * This probe runs as init on a disk whose ext4 occupies only the head
 * (boot-time swap_init carves the SWAP_SIZE_MB tail — 256 MB at the
 * default config) and proves the path behaviorally:
 *
 *   1. /proc/meminfo shows SwapTotal > 0        (swap area active);
 *   2. anonymous pressure drives SwapFree DOWN   (try_to_unmap_with_swap
 *      actually replaced PTEs — the FD-1 observable);
 *   3. every page of the touched arena reads back its pattern afterwards
 *      (the leaf slot written was the RIGHT one; swap-in restores data —
 *      a mis-wired walk corrupts or kills here);
 *   4. serial stays clean and the probe powers off.
 *
 * Final line: "PROBE-RESULT: PASS|FAIL <detail>".
 */

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/reboot.h>
#include <unistd.h>

#define RESERVE_MB 96u            /* kernel + static bins kept resident */

static unsigned long meminfo_kb(const char *key)
{
    static char buf[16384];
    int fd = open("/proc/meminfo", O_RDONLY);
    if (fd < 0)
        return 0;
    int total = 0, n;
    while (total < (int)sizeof(buf) - 1 &&
           (n = read(fd, buf + total, sizeof(buf) - 1 - total)) > 0)
        total += n;
    close(fd);
    buf[total] = 0;

    size_t klen = strlen(key);
    for (char *line = strtok(buf, "\n"); line; line = strtok(NULL, "\n")) {
        if (strncmp(line, key, klen) == 0 && line[klen] == ':') {
            unsigned long v = 0;
            sscanf(line + klen + 1, "%lu", &v);
            return v;
        }
    }
    return 0;
}

static void power_off(void)
{
    fflush(stdout);
    sleep(1);
    reboot(0x4321fedc); /* LINUX_REBOOT_CMD_POWER_OFF (the dfx-x86 idiom) */
    _exit(0);
}

int main(void)
{
    printf("===SWAP-X86: start\n");
    fflush(stdout);

    /* 1. Swap area active? (boot swap_init carved the disk tail) */
    unsigned long swap_total = meminfo_kb("SwapTotal");
    unsigned long swap_free0 = meminfo_kb("SwapFree");
    printf("swap-x86: SwapTotal=%lu kB SwapFree=%lu kB\n",
           swap_total, swap_free0);
    fflush(stdout);
    if (swap_total == 0) {
        printf("PROBE-RESULT: FAIL swap area not active (SwapTotal=0)\n");
        power_off();
    }

    unsigned long mem_total = meminfo_kb("MemTotal");
    if (mem_total < 320u * 1024) {
        printf("PROBE-RESULT: SKIP MemTotal=%lu kB too small to force reclaim\n",
               mem_total);
        power_off();
    }

    /* 2. Anonymous pressure: one big MAP_PRIVATE|MAP_ANON arena slightly
     * PAST what fits resident (MemFree + 128 MB — well inside the 256 MB
     * swap budget, so the OOM killer has no excuse if reclaim works).
     * Sequential touching leaves the head cold — the inactive-anon LRU
     * scan class vmscan swaps out. */
    unsigned long mem_free = meminfo_kb("MemFree");
    unsigned long target_kb = mem_free + 128u * 1024;
    if (target_kb > mem_total - 32u * 1024)
        target_kb = mem_total - 32u * 1024;
    printf("swap-x86: MemTotal=%lu kB MemFree=%lu kB target arena=%lu kB\n",
           mem_total, mem_free, target_kb);
    fflush(stdout);

    unsigned char *arena = mmap(NULL, target_kb << 10, PROT_READ | PROT_WRITE,
                                MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (arena == MAP_FAILED) {
        printf("PROBE-RESULT: FAIL arena mmap errno=%d\n", errno);
        power_off();
    }
    unsigned long arena_kb = target_kb;

    /* Fill pattern: each 4K page carries its page index in the first
     * word and a per-page checksum in the last. */
    for (unsigned long off = 0; off < (arena_kb << 10); off += 4096) {
        unsigned long page = off / 4096;
        unsigned long *p = (unsigned long *)(arena + off);
        p[0] = page;
        p[511] = page * 2654435761u + 12345;
    }
    printf("swap-x86: touched %lu kB anonymous\n", arena_kb);
    fflush(stdout);

    /* 3. The FD-1 observable: slots must have been consumed. */
    unsigned long swap_free1 = meminfo_kb("SwapFree");
    unsigned long swapped_kb = (swap_free0 > swap_free1)
                                   ? swap_free0 - swap_free1
                                   : 0;
    printf("swap-x86: SwapFree now %lu kB (swapped out %lu kB)\n",
           swap_free1, swapped_kb);
    fflush(stdout);

    /* 4. Integrity: re-read every page. Swapped-out pages fault back in
     * through the swap entry the unmap installed — wrong slot, lost
     * write, or corrupted entry shows up here. */
    unsigned long bad = 0;
    for (unsigned long off = 0; off < (arena_kb << 10); off += 4096) {
        unsigned long page = off / 4096;
        unsigned long *p = (unsigned long *)(arena + off);
        if (p[0] != page || p[511] != page * 2654435761u + 12345) {
            if (bad < 8)
                printf("swap-x86: CORRUPT page %lu: head=%lu tail=%lu\n",
                       page, p[0], p[511]);
            bad++;
        }
    }
    printf("swap-x86: verify pass done (%lu/%lu pages bad)\n",
           bad, (arena_kb << 10) / 4096);
    fflush(stdout);

    if (bad != 0)
        printf("PROBE-RESULT: FAIL %lu pages corrupted after swap cycle\n",
               bad);
    else if (swapped_kb == 0)
        printf("PROBE-RESULT: FAIL no swap consumed "
               "(try_to_unmap_with_swap replaced no PTE)\n");
    else
        printf("PROBE-RESULT: PASS swapped_out=%lu kB arena=%lu kB "
               "corrupt=0\n",
               swapped_kb, arena_kb);
    power_off();
    return 0;
}
