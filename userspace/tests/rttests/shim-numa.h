/* numa-shim.h — minimal libnuma API subset for building rt-tests
 * (qa-suites lane) on a host without libnuma-dev. Layout-compatible
 * with libnuma's struct bitmask. Semantics follow libnuma where the
 * guest can observe them; numa_available() reports "no NUMA" so
 * rt-tests takes its single-node paths (node == -1 everywhere).
 */
#ifndef QA_RTTESTS_NUMA_SHIM_H
#define QA_RTTESTS_NUMA_SHIM_H

#include <stddef.h>
#include <sys/types.h>

struct bitmask {
    unsigned long size; /* number of bits in the map */
    unsigned long *maskp;
};

int numa_available(void);
struct bitmask *numa_allocate_cpumask(void);
void numa_bitmask_free(struct bitmask *bmp);
unsigned int numa_bitmask_weight(const struct bitmask *bmp);
int numa_bitmask_isbitset(const struct bitmask *bmp, unsigned int n);
void numa_bitmask_setbit(struct bitmask *bmp, unsigned int n);
void numa_bitmask_clearbit(struct bitmask *bmp, unsigned int n);
int numa_sched_setaffinity(pid_t pid, struct bitmask *bmp);
int numa_sched_getaffinity(pid_t pid, struct bitmask *bmp);
struct bitmask *numa_parse_cpustring_all(const char *s);
void *numa_alloc_onnode(size_t size, int node);
void numa_free(void *ptr, size_t size);
int numa_run_on_node(int node);
int numa_node_of_cpu(int cpu);

#endif
