/* openat2_probe.c — raw-syscall openat2 RESOLVE_* probe.
 * mkdir /tmp/o2t; create foo; symlink foo->foo_lnk; chdir; then:
 *   BENEATH   /proc/version  -> EXDEV
 *   NO_SYMLNK foo_lnk        -> ELOOP
 *   NO_XDEV   /proc/version  -> EXDEV
 *   IN_ROOT   /proc/version  -> ENOENT
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

struct open_how { u64 flags, mode, resolve; };

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

static i64 o2(const char *path, u64 resolve)
{
    struct open_how how = { 0 /*O_RDONLY*/, 0, resolve };
    return sys5(437, -100, (i64)path, (i64)&how, 24, 0);
}

void _start(void)
{
    puts_("O2-PROBE-START");
    sys3(34, -100, (i64)"/tmp/o2t", 0777);           /* mkdirat */
    i64 fd = sys5(56, -100, (i64)"/tmp/o2t/foo", 0x41, 0644, 0);
    sys3(57, fd, 0, 0);                              /* close */
    i64 sl = sys5(36, (i64)"foo", -100, (i64)"/tmp/o2t/foo_lnk", 0, 0); /* symlinkat */
    puts_("symlink rc:");
    putd(sl);
    i64 cd = sys3(49, (i64)"/tmp/o2t", 0, 0);        /* chdir */
    puts_("chdir rc:");
    putd(cd);
    char cwdbuf[128];
    i64 cwd = sys5(17, (i64)cwdbuf, 128, 0, 0, 0);   /* getcwd */
    puts_("getcwd rc:");
    putd(cwd);
    puts_("cwd:");
    puts_(cwdbuf);

    puts_("beneath /proc/version rc (want -18):");
    putd(o2("/proc/version", 0x08));
    puts_("nosymlink foo_lnk rc (want -40):");
    putd(o2("foo_lnk", 0x02));
    puts_("noxdev /proc/version rc (want -18):");
    putd(o2("/proc/version", 0x01));
    puts_("inroot /proc/version rc (want -2):");
    putd(o2("/proc/version", 0x10));
    puts_("plain foo rc (want >=0):");
    putd(o2("foo", 0));

    puts_("O2-PROBE-DONE");
    sys3(93, 0, 0, 0);
    __builtin_unreachable();
}
