// munmapwrap.c — deterministic length-wrap repro for the s15 catch
// (ltp-a r22 panic: `attempt to add with overflow` at
// arch/riscv64/mm/mm_ops.rs:594, input munmap(0, 0xfffffffffffff000)
// from madvise02's full-range cleanup).
//
// Expected kernel behavior (Linux semantics): every range that wraps
// or exceeds the user address space returns EINVAL; a normal aligned
// map/unmap pair succeeds. On the pre-fix debug kernel the first call
// panics the box, which is the r22 capture.
#include <sys/mman.h>
#include <errno.h>
#include <stdio.h>

static int failures;

static void expect_einval(const char *what, void *addr, unsigned long len)
{
    errno = 0;
    int rc = munmap(addr, len);
    if (rc == 0) {
        printf("PROBE-FAIL %s: munmap succeeded, expected EINVAL\n", what);
        failures++;
    } else if (errno != EINVAL) {
        printf("PROBE-FAIL %s: errno=%d, expected EINVAL\n", what, errno);
        failures++;
    } else {
        printf("PROBE-PASS %s: EINVAL\n", what);
    }
}

int main(void)
{
    // The exact r22 input: page-aligned len one page below usize::MAX.
    // The pre-fix page-round `(size + 4096) - 1` overflows on the add.
    expect_einval("r22-len fffffffffffff000", (void *)0, 0xfffffffffffff000UL);
    // len = -1: any round-up arithmetic wraps.
    expect_einval("len ffffffffffffffff", (void *)0, 0xffffffffffffffffUL);
    // unaligned near-MAX len: wraps AND is unaligned.
    expect_einval("len fffffffffffff001", (void *)0, 0xfffffffffffff001UL);
    // huge-but-wrappable range at a nonzero base (R7-1 end bound).
    expect_einval("base+wrap", (void *)0x3000000000UL, 0xfffffffffff00000UL);

    // Control: a real mapping unmaps cleanly.
    void *p = mmap((void *)0, 0x200000, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) {
        printf("PROBE-FAIL control mmap: errno=%d\n", errno);
        failures++;
    } else if (munmap(p, 0x200000) != 0) {
        printf("PROBE-FAIL control munmap: errno=%d\n", errno);
        failures++;
    } else {
        printf("PROBE-PASS control map/unmap\n");
    }

    if (failures == 0)
        printf("MUNMAPWRAP-VERDICT PASS all classes\n");
    else
        printf("MUNMAPWRAP-VERDICT FAIL %d classes\n", failures);
    return failures ? 1 : 0;
}
