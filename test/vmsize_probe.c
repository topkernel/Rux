/* vmsize_probe.c — the virtual-size accounting must follow the LIVE
 * mapping.
 *
 * The reference kernel keeps mm->total_vm equal to the live mapping size
 * (kernel/mm/mmap.c: vm_stat_account on install, -nrpages on removal,
 * may_expand_vm against it for RLIMIT_AS). /proc/<pid>/status reports it
 * as VmSize.
 *
 * A counter that only grows is wrong in both directions: it refuses
 * mappings that fit once the process has mapped and unmapped enough, and
 * it reports a VmSize that describes every mapping the process ever made
 * instead of the ones it holds.
 *
 * The probe maps 4 MB, checks that VmSize grew by at least that much,
 * unmaps it, and checks that the mapping's pages are gone again. The
 * "back to the baseline" comparison allows a small slack: the probe's own
 * stdio buffer makes the process trim and re-grow its heap between the
 * readings, and that churn is the process's own business — the mapping
 * must not be left behind.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define MAP_LEN (4UL << 20)   /* 4 MB */
#define MAP_KB (MAP_LEN >> 10)
#define SLACK_KB 256          /* the process's own heap churn */

static long vm_size_kb(void)
{
    FILE *f = fopen("/proc/self/status", "r");
    char line[256];
    long kb = -1;

    if (!f)
        return -1;
    while (fgets(line, sizeof(line), f)) {
        if (strncmp(line, "VmSize:", 7) == 0) {
            kb = strtol(line + 7, NULL, 10);
            break;
        }
    }
    fclose(f);
    return kb;
}

int main(void)
{
    long before, during, after;

    before = vm_size_kb();
    if (before < 0) {
        printf("VMSIZE-PROBE: FAIL no /proc/self/status VmSize\n");
        return 1;
    }

    void *p = mmap(NULL, MAP_LEN, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) {
        printf("VMSIZE-PROBE: FAIL mmap\n");
        return 1;
    }

    during = vm_size_kb();
    if (during - before < (long)MAP_KB) {
        printf("VMSIZE-PROBE: FAIL mmap not charged: %ld -> %ld kB\n",
               before, during);
        return 1;
    }

    if (munmap(p, MAP_LEN) != 0) {
        printf("VMSIZE-PROBE: FAIL munmap\n");
        return 1;
    }

    after = vm_size_kb();

    /* The mapping's own pages must be gone again ... */
    if (during - after < (long)MAP_KB) {
        printf("VMSIZE-PROBE: FAIL munmap did not release the mapping: "
               "%ld -> %ld -> %ld kB\n", before, during, after);
        return 1;
    }
    /* ... and nothing of the mapping may be left behind (only the
     * process's own heap churn may differ). */
    if (after - before > SLACK_KB) {
        printf("VMSIZE-PROBE: FAIL the mapping is still accounted: "
               "%ld -> %ld -> %ld kB\n", before, during, after);
        return 1;
    }

    printf("VMSIZE-PROBE: PASS %ld -> %ld -> %ld kB\n", before, during, after);
    return 0;
}
