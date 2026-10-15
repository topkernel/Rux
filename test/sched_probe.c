/* sched_probe.c — SCHED_RESET_ON_FORK semantics probe (raw syscalls).
 * riscv64: sched_setscheduler=119 sched_getscheduler=120 sched_getparam=121
 * clone=220 nanosleep=35 getpid=172
 *
 * Parent: sched_setscheduler(0, SCHED_FIFO|RESET_ON_FORK, prio 10) -> rc
 * Fork; child sleeps 4s (stays alive). Parent reads child's policy/prio.
 * Expect: child policy 0 (SCHED_NORMAL), priority 0.
 */
typedef unsigned long u64;
typedef long i64;

static inline i64 sys3(i64 n, i64 a, i64 b, i64 c)
{
    register i64 a0 asm("a0") = a;
    register i64 a1 asm("a1") = b;
    register i64 a2 asm("a2") = c;
    register i64 a7 asm("a7") = n;
    asm volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a7) : "memory");
    return a0;
}
static inline i64 sys5(i64 n, i64 a, i64 b, i64 c, i64 d, i64 e)
{
    register i64 a0 asm("a0") = a;
    register i64 a1 asm("a1") = b;
    register i64 a2 asm("a2") = c;
    register i64 a3 asm("a3") = d;
    register i64 a4 asm("a4") = e;
    register i64 a7 asm("a7") = n;
    asm volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a7) : "memory");
    return a0;
}

struct ts { i64 sec, nsec; };

static void puts_(const char *s)
{
    u64 n = 0;
    while (s[n]) n++;
    sys3(64, 1, (i64)s, (i64)n);
}
static void putd(i64 v)
{
    char b[24], r[24];
    int n = 0;
    if (v < 0) { sys3(64, 1, (i64)"-", 1); v = -v; }
    if (v == 0) b[n++] = '0';
    while (v > 0) { b[n++] = '0' + (v % 10); v /= 10; }
    for (int i = 0; i < n; i++) r[i] = b[n - 1 - i];
    sys3(64, 1, (i64)r, n);
    sys3(64, 1, (i64)"\n", 1);
}

void _start(void)
{
    puts_("SCHED-PROBE-START");
    /* SCHED_FIFO=1, RESET_ON_FORK=0x40000000, prio=10 */
    i64 rc = sys5(119, 0, 1 | 0x40000000, (i64)&(struct { i64 p; }){10}, 0, 0);
    puts_("setscheduler rc:");
    putd(rc);

    i64 pol = sys3(120, 0, 0, 0);
    puts_("parent policy:");
    putd(pol);

    i64 pid = sys5(220 /*clone*/, 17 /*SIGCHLD*/, 0, 0, 0, 0);
    if (pid == 0) {
        /* child: nap 4s so the parent can inspect us */
        struct ts t = { 4, 0 };
        sys3(35, (i64)&t, 0, 0);
        sys3(93, 0, 0, 0);
        __builtin_unreachable();
    }
    puts_("child pid:");
    putd(pid);

    i64 cpol = sys3(120, pid, 0, 0);
    puts_("child policy (want 0):");
    putd(cpol);

    i64 cprio = sys3(121, pid, (i64)&(struct { i64 p; }){0}, 0);
    puts_("child getparam rc:");
    putd(cprio);

    /* reap */
    sys3(64, 1, (i64)"SCHED-PROBE-DONE\n", 17);
    sys3(93, 0, 0, 0);
    __builtin_unreachable();
}
