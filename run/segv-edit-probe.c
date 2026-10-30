/*
 * segv-edit-probe.c — synchronous-signal roundtrip probe (aa64-signal).
 *
 * Self-inflicted SIGSEGV (store to page 0) caught by an SA_SIGINFO
 * handler that EDITS the frame's ucontext (mcontext.pc advanced past
 * the faulting instruction, x0 set to a magic). This exercises, with
 * no dependence on pid lookups or working IRQs:
 *   - page-fault -> SIGSEGV routing (fault_sigsegv_route)
 *   - SA_SIGINFO argument passing (x1=&siginfo, x2=&ucontext)
 *   - the frame being readable AND writable at its ABI offsets
 *   - rt_sigreturn honoring USER-EDITED mcontext (M-04 contract)
 * Park leaves the verdict in the MB mailbox (see v0-probe4).
 */
#include <stdint.h>
#define __NR_rt_sigaction 134
static long sys3(long n, long a, long b, long c, long d) {
    register long x8 __asm__("x8") = n;
    register long x0 __asm__("x0") = a;
    register long x1 __asm__("x1") = b;
    register long x2 __asm__("x2") = c;
    register long x3 __asm__("x3") = d;
    __asm__ volatile("svc #0" : "+r"(x0) : "r"(x8), "r"(x1), "r"(x2), "r"(x3) : "memory", "cc");
    return x0;
}

static volatile uint64_t MB[8];
#define TAG(i, v) (((uint64_t)(i) << 48) | ((uint64_t)(int64_t)(v) & 0xFFFFFFFFFFFFULL))

static volatile uint64_t g_count;
static volatile uint64_t g_code;
static volatile uint64_t g_addr;

/* fault trigger: str xzr,[xzr] (4 bytes) then mov x0,#1; ret */
__attribute__((noinline)) static long trigger(void) {
    long r = 0;
    __asm__ volatile(
        "mov x9, xzr\n"
        "str x9, [x9]\n"     /* faults: SIGSEGV; ELR = THIS insn */
        "mov %0, #1\n"       /* skipped by the handler's pc += 8 */
        : "=r"(r) :: "x9");
    return r;
}

/* siginfo: si_code @8 (kernel ABI header). The frame's ucontext:
 * uc_mcontext @176; inside sigcontext: fault_address @176, x0..x30
 * @184..432, sp @432, pc @440, pstate @448 (arm64 uapi). */
static void handler(int sig, void *info, void *ucv)
{
    (void)sig;
    unsigned char *u = (unsigned char *)ucv;
    uint64_t *pc = (uint64_t *)(u + 440);
    g_code = *(int *)((unsigned char *)info + 8);
    g_addr = *(uint64_t *)((unsigned char *)info + 16); /* sifields rux value slot */
    /* skip the faulting store AND the following mov: pc += 8 */
    *pc = *pc + 8;
    g_count++;
}
struct kernel_sigaction { void *handler; uint64_t flags; void *restorer; uint64_t mask; };
#define SA_SIGINFO_ 0x00000004

void _start(void)
{
    struct kernel_sigaction sa = { (void *)handler, SA_SIGINFO_, 0, 0 };
    struct kernel_sigaction old;
    long ra = sys3(__NR_rt_sigaction, 11 /*SIGSEGV*/, (long)&sa, (long)&old, 8);
    const int rounds = 5;
    long last = -1;
#ifdef NOFAULT
    (void)trigger; (void)rounds; (void)last;
#else
    for (int i = 0; i < rounds; i++) {
        last = trigger();   /* returns 0 via the pc edit (mov skipped) */
    }
#endif
    MB[0] = TAG(1, ra);
    MB[1] = TAG(2, g_count);
    MB[2] = TAG(3, g_code);
    MB[3] = TAG(4, g_addr);
    MB[4] = TAG(5, last);
    __asm__ volatile(".global park_segv\npark_segv:\n"
        "1: adrp x9, MB\n"
        "   add x9, x9, :lo12:MB\n"
        "   ldp x10, x11, [x9, #0]\n"
        "   ldp x12, x13, [x9, #16]\n"
        "   ldr x14, [x9, #32]\n"
        "   b 1b\n");
    __builtin_unreachable();
}
