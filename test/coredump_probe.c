// MIT License
//
// Copyright (c) 2026 Fei Wang
//
// Core dump E2E probe (static glibc — riscv64-linux-gnu-gcc -static).
//
// Flow:
//   1. setrlimit(RLIMIT_CORE, infinity) — the kernel default soft limit
//      is 0 (Linux INIT_RLIMITS parity), so the probe opts in first.
//      Prints the pre/post values so the gate on RLIMIT_CORE is itself
//      exercised.
//   2. Writes a marker string into .data and pushes recognizable values
//      into callee-saved registers (s1..s3) right before the fault, so
//      the core file can be checked for real register + memory state.
//   3. Dereferences NULL -> SIGSEGV -> kernel default action -> core
//      file named by /proc/sys/kernel/core_pattern ("core.%p" default)
//      in the current working directory.
//
// Host-side verification:
//   riscv64-linux-gnu-objdump -x core.<pid>          (segments + notes)
//   gdb-multiarch ./coredump_probe --core core.<pid> (regs + symbolized PC)
//
// Exit status is meaningless — the probe dies by signal on success.

#include <stdio.h>
#include <string.h>
#include <sys/resource.h>

static char probe_data[64] = "RUX-COREDUMP-PROBE-DATA-V1";

/* fault_here keeps the faulting PC in a symbol of its own so the
 * symbolized crash PC check is exact. */
static void __attribute__((noinline)) fault_here(void)
{
    /* Callee-saved markers: s1/s2/s3 = 0xR1/0xR2/0xR3 */
    __asm__ volatile(
        "li s1, 0x52551\n"
        "li s2, 0x52552\n"
        "li s3, 0x52553\n"
        ::: "s1", "s2", "s3");
    printf("probe: about to dereference NULL\n");
    fflush(stdout);
    *(volatile unsigned long *)0 = 0xdeadbeef;
    /* not reached */
    printf("probe: ERROR - survived NULL deref\n");
}

int main(void)
{
    struct rlimit before, infinite = { RLIM_INFINITY, RLIM_INFINITY };

    if (getrlimit(RLIMIT_CORE, &before) == 0)
        printf("probe: RLIMIT_CORE before = cur=%llu max=%llu\n",
               (unsigned long long)before.rlim_cur,
               (unsigned long long)before.rlim_max);
    if (setrlimit(RLIMIT_CORE, &infinite) != 0) {
        printf("probe: ERROR - setrlimit(RLIMIT_CORE) failed\n");
        return 1;
    }
    printf("probe: RLIMIT_CORE raised to infinity\n");
    printf("probe: data marker at %p: %s\n", (void *)probe_data, probe_data);
    fflush(stdout);

    fault_here();
    return 0;
}
