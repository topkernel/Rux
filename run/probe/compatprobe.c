/* compatprobe — syscall-only probe (no libc) for the compat-review lane.
 * Covers: symlink-ring ELOOP arithmetic (rmdir/readlink/newfstatat),
 * clone flag combos, rt_sigtimedwait wake, sched_[gs]etaffinity. */

static long sys(long nr, long a, long b, long c, long d, long e, long f) {
    register long a0 asm("a0") = a;
    register long a1 asm("a1") = b;
    register long a2 asm("a2") = c;
    register long a3 asm("a3") = d;
    register long a4 asm("a4") = e;
    register long a5 asm("a5") = f;
    register long a7 asm("a7") = nr;
    asm volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a5), "r"(a7) : "memory");
    return a0;
}
#define SYS_openat 56
#define SYS_close 57
#define SYS_lseek 62
#define SYS_read 63
#define SYS_write 64
#define SYS_readlinkat 78
#define SYS_newfstatat 79
#define SYS_exit 93
#define SYS_exit_group 94
#define SYS_waitid 95
#define SYS_kill 129
#define SYS_clone 220
#define SYS_munmap 215
#define SYS_rt_sigtimedwait 137
#define SYS_rt_sigprocmask 135
#define SYS_mkdirat 34
#define SYS_symlinkat 36
#define SYS_unlinkat 35
#define SYS_sched_getaffinity 123
#define SYS_sched_setaffinity 122

static void w(const char *s) { while (*s) sys(SYS_write, 1, (long)s++, 1, 0, 0, 0); }
static void wn(long v) {
    char b[24]; int i = 23; b[i--] = 0;
    int neg = v < 0; unsigned long u = neg ? -v : v;
    do { b[i--] = '0' + (u % 10); u /= 10; } while (u);
    if (neg) b[i--] = '-';
    w(&b[i+1]);
}
static void werr(void) { /* print errno-ish: raw return */ }

static char ring[8192];
static char g_buf[256];

static long g_mkdir_rc, g_symlink_rc, g_cwd;
static void test_ring(void) {
    /* LTP rmdir02 construction: loopdir real dir + loopdir/loopdir -> ../loopdir */
    g_cwd = sys(79 /*newfstatat*/, -100, (long)".", (long)g_buf, 0, 0, 0);
    g_mkdir_rc = sys(SYS_mkdirat, -100, (long)"loopdir", 0755, 0, 0, 0);
    sys(SYS_unlinkat, -100, (long)"loopdir/loopdir", 0, 0, 0, 0);
    g_symlink_rc = sys(36 /*symlinkat*/, (long)"../loopdir", -100, (long)"loopdir/loopdir", 0, 0, 0);
    static char out[256];
    for (int n = 38; n <= 46; n++) {
        int len = 0;
        for (int i = 0; i < n; i++) { ring[len++] = 'l'; ring[len++]='o'; ring[len++]='o'; ring[len++]='p'; ring[len++]='d'; ring[len++]='i'; ring[len++]='r'; ring[len++]='/'; }
        ring[len] = 0;
        long r1 = sys(35, -100, (long)ring, 0x200, 0, 0, 0);
        long r2 = sys(SYS_readlinkat, -100, (long)ring, (long)out, 200, 0, 0);
        long r3 = sys(SYS_newfstatat, -100, (long)ring, (long)out, 0, 0, 0);
        w("n="); wn(n);
        w("|u="); wn(r1);
        w("|rl="); wn(r2);
        w("|st="); wn(r3);
        w("\n");
    }
}

static int child(void) { sys(SYS_exit, 0, 0, 0, 0, 0, 0); return 0; }
static char cstack[65536] __attribute__((aligned(16)));

static void test_clone(void) {
    /* minimum legal thread set and clone08's exact set */
    static const long vA = 0x00010000 | 0x00000800 | 0x00000100;         /* THREAD|SIGHAND|VM */
    static const long vB = vA | 0x00000200 | 0x00000400 | 0x00100000 | 0x00200000; /* +FS|FILES|PARENT_SETTID|CHILD_CLEARTID */
    long variants[2] = { vA, vB };
    const char *names[2] = { "A:THREAD|SIGHAND|VM", "B:+FS|FILES|SETTID" };
    long ctid = -1, ptid = 0;
    for (int v = 0; v < 2; v++) {
        ctid = -1;
        long r = sys(SYS_clone, variants[v] | 17 /*SIGCHLD*/, (long)(cstack + sizeof(cstack)), (long)&ptid, 0, (long)&ctid, 0);
        if (r == 0) { sys(SYS_exit, 0, 0, 0, 0, 0, 0); }
        w("clone "); w(names[v]); w(" = "); wn(r);
        w(" ptid="); wn(ptid); w(" ctid="); wn(ctid); w("\n");
        if (r > 0) { for (volatile int i = 0; i < 50000000; i++) {} }
    }
}

static void test_sigtimedwait(void) {
    /* block SIGUSR1(10), child sends after a spin, parent rt_sigtimedwait */
    long set = 0, old = 0;
    set |= 1L << 9; /* SIGUSR1 - 1 */
    long r = sys(SYS_rt_sigprocmask, 0 /*BLOCK*/, (long)&set, (long)&old, 8, 0, 0);
    w("sigprocmask="); wn(r); w("\n");
    long pid = sys(SYS_clone, 17 /*SIGCHLD*/, (long)(cstack + sizeof(cstack)), 0, 0, 0, 0);
    if (pid == 0) {
        long ppid = 1;
        /* getppid = 173 */
        ppid = sys(173, 0, 0, 0, 0, 0, 0);
        for (volatile int i = 0; i < 30000000; i++) {}
        sys(SYS_kill, ppid, 10, 0, 0, 0, 0);
        sys(SYS_exit, 0, 0, 0, 0, 0, 0);
    }
    struct ts { long sec; long nsec; } ts = { 8, 0 };
    static char sinfo[128];
    r = sys(SYS_rt_sigtimedwait, (long)&set, (long)sinfo, (long)&ts, 8, 0, 0);
    w("rt_sigtimedwait="); wn(r);
    /* si_signo at offset 0 of siginfo */
    w(" si_signo="); wn(*(long *)sinfo); w("\n");
    for (volatile int i = 0; i < 100000000; i++) {}
}

static void test_affinity(void) {
    static unsigned char big[128];
    for (int i = 0; i < 128; i++) big[i] = 0;
    long r = sys(SYS_sched_getaffinity, 0, 128, (long)big, 0, 0, 0);
    unsigned long long m = 0;
    for (int i = 0; i < 8; i++) m |= (unsigned long long)big[i] << (8 * i);
    w("getaffinity(128)="); wn(r); w(" mask="); 
    /* print hex */
    static const char hx[] = "0123456789abcdef";
    char hb[19]; hb[0]='0'; hb[1]='x';
    for (int i = 15; i >= 0; i--) { hb[2 + (15 - i)] = hx[(m >> (4 * i)) & 0xf]; }
    hb[18] = 0; w(hb); w("\n");
    int maxcpu = 0;
    for (int i = 0; i < 128 * 8; i++) if (big[i / 8] & (1 << (i % 8))) maxcpu = i;
    for (int i = 0; i < 128; i++) big[i] = 0;
    big[maxcpu / 8] |= 1 << (maxcpu % 8);
    r = sys(SYS_sched_setaffinity, 0, 128, (long)big, 0, 0, 0);
    w("setaffinity(max="); wn(maxcpu); w(")="); wn(r); w("\n");
}

void _start(void) {
    asm volatile(".option push\n\t.option norelax\n\tla gp, __global_pointer$\n\t.option pop" ::: "memory");
    w("== compatprobe begin\n");
    test_affinity();
    test_clone();
    test_sigtimedwait();
    test_ring();
    w("mkdir="); wn(g_mkdir_rc); w(" symlink="); wn(g_symlink_rc); w(" statcwd="); wn(g_cwd); w("\n");
    w("== compatprobe done\n");
    sys(SYS_exit_group, 0, 0, 0, 0, 0, 0);
    __builtin_unreachable();
}
