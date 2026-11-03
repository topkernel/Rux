/* numa-shim.c — minimal libnuma implementation for rt-tests static
 * builds (qa-suites lane). The guest is a single-NUMA-node QEMU virt
 * machine: numa_available() returns -1 so rt-tests runs its
 * single-node code paths, and the affinity calls map onto
 * sched_setaffinity/sched_getaffinity exactly like libnuma does.
 */
#include <errno.h>
#include <sched.h>
#include <stdlib.h>
#include <string.h>
#include <ctype.h>
#include "numa.h"

#define SHIM_MAX_CPUS 1024

int numa_available(void)
{
    return -1; /* single node: rt-tests falls back to plain malloc/affinity */
}

static struct bitmask *bmp_alloc(unsigned int nbits)
{
    struct bitmask *bmp = calloc(1, sizeof(*bmp));
    if (!bmp)
        return NULL;
    bmp->size = nbits;
    bmp->maskp = calloc((nbits + 63) / 64 + 1, sizeof(unsigned long));
    if (!bmp->maskp) {
        free(bmp);
        return NULL;
    }
    return bmp;
}

struct bitmask *numa_allocate_cpumask(void)
{
    return bmp_alloc(SHIM_MAX_CPUS);
}

void numa_bitmask_free(struct bitmask *bmp)
{
    if (bmp) {
        free(bmp->maskp);
        free(bmp);
    }
}

unsigned int numa_bitmask_weight(const struct bitmask *bmp)
{
    unsigned int w = 0;
    if (!bmp)
        return 0;
    for (unsigned int i = 0; i < bmp->size; i++)
        if (numa_bitmask_isbitset(bmp, i))
            w++;
    return w;
}

int numa_bitmask_isbitset(const struct bitmask *bmp, unsigned int n)
{
    if (!bmp || n >= bmp->size)
        return 0;
    return (bmp->maskp[n / 64] >> (n % 64)) & 1UL;
}

void numa_bitmask_setbit(struct bitmask *bmp, unsigned int n)
{
    if (!bmp || n >= bmp->size)
        return;
    bmp->maskp[n / 64] |= 1UL << (n % 64);
}

void numa_bitmask_clearbit(struct bitmask *bmp, unsigned int n)
{
    if (!bmp || n >= bmp->size)
        return;
    bmp->maskp[n / 64] &= ~(1UL << (n % 64));
}

static void bmp_to_cpuset(const struct bitmask *bmp, cpu_set_t *set)
{
    CPU_ZERO(set);
    if (!bmp)
        return;
    for (unsigned int i = 0; i < bmp->size && i < CPU_SETSIZE; i++)
        if (numa_bitmask_isbitset(bmp, i))
            CPU_SET(i, set);
}

static void cpuset_to_bmp(const cpu_set_t *set, struct bitmask *bmp)
{
    if (!bmp)
        return;
    for (unsigned int i = 0; i < CPU_SETSIZE && i < bmp->size; i++)
        if (CPU_ISSET(i, set))
            numa_bitmask_setbit(bmp, i);
}

int numa_sched_setaffinity(pid_t pid, struct bitmask *bmp)
{
    cpu_set_t set;
    bmp_to_cpuset(bmp, &set);
    return sched_setaffinity(pid, sizeof(set), &set);
}

int numa_sched_getaffinity(pid_t pid, struct bitmask *bmp)
{
    cpu_set_t set;
    if (sched_getaffinity(pid, sizeof(set), &set) < 0)
        return -1;
    cpuset_to_bmp(&set, bmp);
    return 0;
}

struct bitmask *numa_parse_cpustring_all(const char *s)
{
    struct bitmask *bmp = numa_allocate_cpumask();
    const char *p = s;
    if (!bmp)
        return NULL;
    while (*p) {
        unsigned long a, b;
        char *end;
        if (!isdigit((unsigned char)*p)) {
            numa_bitmask_free(bmp);
            return NULL;
        }
        a = strtoul(p, &end, 10);
        p = end;
        b = a;
        if (*p == '-') {
            p++;
            if (!isdigit((unsigned char)*p)) {
                numa_bitmask_free(bmp);
                return NULL;
            }
            b = strtoul(p, &end, 10);
            p = end;
        }
        for (unsigned long i = a; i <= b && i < bmp->size; i++)
            numa_bitmask_setbit(bmp, (unsigned int)i);
        if (*p == ',')
            p++;
    }
    return bmp;
}

void *numa_alloc_onnode(size_t size, int node)
{
    (void)node; /* single node: plain heap */
    return malloc(size);
}

void numa_free(void *ptr, size_t size)
{
    (void)size;
    free(ptr);
}

int numa_run_on_node(int node)
{
    if (node != -1) {
        errno = EINVAL;
        return -1;
    }
    return 0;
}

int numa_node_of_cpu(int cpu)
{
    return cpu >= 0 && cpu < SHIM_MAX_CPUS ? 0 : -1;
}
