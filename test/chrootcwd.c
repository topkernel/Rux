/* chrootcwd — freestanding chroot/getcwd/cwd-vs-MS_MOVE semantics probe
 * for Rux (chroot-path lane acceptance).
 *
 * Raw syscalls only (no libc), riscv64 ecall convention. Exercises the
 * Linux-parity rules around the mount-aware build_path()/chroot fix:
 *
 *   R1  chdir("/proc"); getcwd == "/proc"      (cross-mount climb kept —
 *                                                the original `cd /proc` bug)
 *   R2  chroot(jail) WITHOUT chdir: getcwd fails ENOENT (cwd outside the
 *       root — historic Linux behavior, "(unreachable)" in modern kernels)
 *   R3  chdir("/") after chroot: getcwd == "/"
 *   R4  chdir("bin") in the jail: getcwd == "/bin" — ROOT-RELATIVE, not
 *       the global "/tmp/jail/bin"
 *   R5  open("/bin/<file>") inside the jail resolves IN-ROOT (the OH
 *       switch_root failure mode: absolute opens going global → ENOENT)
 *   R7  the full switch_root shape on a tmpfs (in a forked child, since
 *       a chroot cannot be exited): mount tmpfs → chdir(mnt) → MS_MOVE
 *       the mount elsewhere → the cwd string must SELF-HEAL from the
 *       dentry (getcwd reports the NEW location, not the stale one) →
 *       chroot(".") (busybox/OH idiom) → getcwd "/" → chdir("sub") →
 *       getcwd "/sub" → absolute open in-root
 *
 * Prints "CHROOTCWD PASS" or "CHROOTCWD FAIL: <case>" and exits non-zero.
 */
typedef unsigned long u64;
typedef long s64;

#define __NR_getcwd 17
#define __NR_mount 40
#define __NR_mkdirat 34
#define __NR_chdir 49
#define __NR_chroot 51
#define __NR_openat 56
#define __NR_close 57
#define __NR_read 63
#define __NR_write 64
#define __NR_exit 93
#define __NR_exit_group 94
#define __NR_clone 220
#define __NR_wait4 260

#define AT_FDCWD (-100L)
#define O_RDONLY 0
#define O_WRONLY 1
#define O_CREAT 0100
#define O_TRUNC 01000
#define MS_MOVE 8192L
#define SIGCHLD 17

#define ENOENT 2

static s64 sys3(s64 n, s64 a, s64 b, s64 c)
{
    register s64 a0 asm("a0") = a;
    register s64 a1 asm("a1") = b;
    register s64 a2 asm("a2") = c;
    register s64 a7 asm("a7") = n;
    asm volatile("ecall"
                 : "+r"(a0)
                 : "r"(a1), "r"(a2), "r"(a7)
                 : "memory");
    return a0;
}

static s64 sys4(s64 n, s64 a, s64 b, s64 c, s64 d)
{
    register s64 a0 asm("a0") = a;
    register s64 a1 asm("a1") = b;
    register s64 a2 asm("a2") = c;
    register s64 a3 asm("a3") = d;
    register s64 a7 asm("a7") = n;
    asm volatile("ecall"
                 : "+r"(a0)
                 : "r"(a1), "r"(a2), "r"(a3), "r"(a7)
                 : "memory");
    return a0;
}

static s64 sys5(s64 n, s64 a, s64 b, s64 c, s64 d, s64 e)
{
    register s64 a0 asm("a0") = a;
    register s64 a1 asm("a1") = b;
    register s64 a2 asm("a2") = c;
    register s64 a3 asm("a3") = d;
    register s64 a4 asm("a4") = e;
    register s64 a7 asm("a7") = n;
    asm volatile("ecall"
                 : "+r"(a0)
                 : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a7)
                 : "memory");
    return a0;
}

static void out(const char *s)
{
    u64 n = 0;
    while (s[n])
        n++;
    sys3(__NR_write, 1, (s64)s, (s64)n);
}

static void outn(s64 v)
{
    char b[24];
    int i = 23;
    int neg = v < 0;
    if (neg)
        v = -v;
    b[i--] = 0;
    do {
        b[i--] = '0' + (char)(v % 10);
        v /= 10;
    } while (v);
    if (neg)
        b[i--] = '-';
    out(&b[i + 1]);
}

static void die(const char *case_name, s64 detail)
{
    out("CHROOTCWD FAIL: ");
    out(case_name);
    out(" (");
    outn(detail);
    out(")\n");
    sys3(__NR_exit_group, 100, 0, 0);
}

/* getcwd into buf; returns >= 0 on success (kernel NUL-terminates), or
 * the negative errno. */
static s64 getcwd(char *buf, u64 size)
{
    return sys3(__NR_getcwd, (s64)buf, (s64)size, 0);
}

static int streq(const char *a, const char *b)
{
    while (*a && *a == *b) {
        a++;
        b++;
    }
    return *a == *b;
}

/* chdir + getcwd, compare against `want`; die(casename) on mismatch. */
static void check_cwd(const char *dir, const char *want, const char *case_name)
{
    s64 r = sys3(__NR_chdir, (s64)dir, 0, 0);
    if (r < 0)
        die(case_name, r);
    char buf[256];
    for (u64 i = 0; i < sizeof(buf); i++)
        buf[i] = 0;
    r = getcwd(buf, sizeof(buf));
    if (r < 0)
        die(case_name, r);
    if (!streq(buf, want))
        die(case_name, 0);
    out("chrootcwd: ");
    out(case_name);
    out(" ok (");
    out(buf);
    out(")\n");
}

/* chdir(dir); getcwd; die(casename) unless it equals `want`. cwd is NOT
 * restored. */
static void expect_cwd(const char *want, const char *case_name)
{
    char buf[256];
    for (u64 i = 0; i < sizeof(buf); i++)
        buf[i] = 0;
    s64 r = getcwd(buf, sizeof(buf));
    if (r < 0)
        die(case_name, r);
    if (!streq(buf, want))
        die(case_name, 0);
    out("chrootcwd: ");
    out(case_name);
    out(" ok (");
    out(buf);
    out(")\n");
}

static void touch(const char *path, const char *content)
{
    s64 fd = sys4(__NR_openat, AT_FDCWD, (s64)path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0)
        die("create file", fd);
    u64 n = 0;
    while (content[n])
        n++;
    s64 r = sys3(__NR_write, fd, (s64)content, (s64)n);
    if (r < 0)
        die("write file", r);
    sys3(__NR_close, fd, 0, 0);
}

static void mkdirp(const char *path)
{
    s64 r = sys4(__NR_mkdirat, AT_FDCWD, (s64)path, 0755, 0);
    /* EEXIST is fine (re-runs of the probe). */
    if (r < 0 && r != -17)
        die("mkdir", r);
}

/* R7: the busybox/OH switch_root idiom on a tmpfs mount, in a child
 * (chroot cannot be exited). */
static void switch_root_child(void)
{
    mkdirp("/tmp/mntsrc");
    mkdirp("/tmp/mvdst");

    s64 r = sys5(__NR_mount, (s64)"none", (s64)"/tmp/mntsrc", (s64)"tmpfs", 0, 0);
    if (r < 0)
        die("R7 mount tmpfs", r);
    mkdirp("/tmp/mntsrc/sub");
    touch("/tmp/mntsrc/sub/inner", "innerfile\n");

    /* chdir into the mount, then MOVE it elsewhere — the cwd string
     * captured at chdir time ("/tmp/mntsrc") is now stale. */
    r = sys3(__NR_chdir, (s64)"/tmp/mntsrc", 0, 0);
    if (r < 0)
        die("R7 chdir mntsrc", r);
    r = sys5(__NR_mount, (s64)"/tmp/mntsrc", (s64)"/tmp/mvdst", 0, MS_MOVE, 0);
    if (r < 0)
        die("R7 MS_MOVE", r);

    /* SELF-HEAL: getcwd must report the NEW location — derived from the
     * cwd dentry, not the stale chdir-time string. */
    expect_cwd("/tmp/mvdst", "R7 cwd self-heal after MS_MOVE");

    /* busybox switch_root: chroot(".") with the cwd on the moved mount. */
    r = sys3(__NR_chroot, (s64)".", 0, 0);
    if (r < 0)
        die("R7 chroot(.)", r);
    expect_cwd("/", "R7 getcwd at new root");

    check_cwd("sub", "/sub", "R7 root-relative subdir");

    /* Absolute open resolves IN-ROOT (the OH boot failure shape). */
    s64 fd = sys4(__NR_openat, AT_FDCWD, (s64)"/sub/inner", O_RDONLY, 0);
    if (fd < 0)
        die("R7 open /sub/inner in-root", fd);
    char rd[16];
    for (u64 i = 0; i < sizeof(rd); i++)
        rd[i] = 0;
    r = sys3(__NR_read, fd, (s64)rd, sizeof(rd) - 1);
    sys3(__NR_close, fd, 0, 0);
    if (r < 0 || !streq(rd, "innerfile\n"))
        die("R7 read content", r);
    out("chrootcwd: R7 absolute open in-root ok\n");

    sys3(__NR_exit_group, 0, 0, 0);
}

void _start(void)
{
    char buf[256];

    /* R1: cross-mount climb — the original `cd /proc` bug. */
    check_cwd("/proc", "/proc", "R1 cd /proc pwd");

    /* R7 in a child: ends chrooted, so it cannot run in this process. */
    s64 child = sys5(__NR_clone, SIGCHLD, 0, 0, 0, 0);
    if (child < 0)
        die("clone", child);
    if (child == 0)
        switch_root_child();
    s64 wstatus = 0;
    s64 r = sys4(__NR_wait4, child, (s64)&wstatus, 0, 0);
    if (r < 0)
        die("wait4", r);
    if (wstatus != 0)
        die("R7 child exit status", wstatus);

    /* --- static jail: /tmp/jail/bin/hello --- */
    mkdirp("/tmp/jail");
    mkdirp("/tmp/jail/bin");
    touch("/tmp/jail/bin/hello", "jailfile\n");

    /* R2: chroot WITHOUT chdir — the cwd (still /proc from R1) is outside
     * the jail → getcwd must fail with ENOENT (cwd == root exactly would
     * legitimately report "/", so do NOT chdir into the jail first). */
    r = sys3(__NR_chroot, (s64)"/tmp/jail", 0, 0);
    if (r < 0)
        die("R2 chroot", r);
    r = getcwd(buf, sizeof(buf));
    if (r != -ENOENT)
        die("R2 getcwd-outside-root-ENOENT", r);
    out("chrootcwd: R2 getcwd outside root ENOENT ok\n");

    /* R3: chdir("/") inside the jail → "/". */
    check_cwd("/", "/", "R3 cd / in jail");

    /* R4: root-relative cwd — "/bin", NOT the global "/tmp/jail/bin". */
    check_cwd("bin", "/bin", "R4 root-relative cwd");

    /* R5: absolute open resolves IN-ROOT (OH failure mode). */
    s64 fd = sys4(__NR_openat, AT_FDCWD, (s64)"/bin/hello", O_RDONLY, 0);
    if (fd < 0)
        die("R5 open /bin/hello in-root", fd);
    char rd[16];
    for (u64 i = 0; i < sizeof(rd); i++)
        rd[i] = 0;
    r = sys3(__NR_read, fd, (s64)rd, sizeof(rd) - 1);
    sys3(__NR_close, fd, 0, 0);
    if (r < 0 || !streq(rd, "jailfile\n"))
        die("R5 read content", r);
    out("chrootcwd: R5 absolute open in-root ok\n");

    out("CHROOTCWD PASS\n");
    sys3(__NR_exit_group, 0, 0, 0);
}
