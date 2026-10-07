/*
 * Minimal freestanding init for the Rux initrd boot test (OH Phase 1).
 *
 * Pure Linux riscv64 syscalls, no libc: prints a banner, dumps
 * /proc/cmdline (verbatim — the kernel must not filter ohos.* tokens),
 * verifies the initrd-unpacked files exist with the right types
 * (regular file, symlink, hard link, dir), then idles in 2-second
 * nanosleep heartbeats so the boot test can also observe dfx=taskdump.
 *
 * Compile: riscv64-linux-gnu-gcc -static -nostdlib -O2 [-DHELLO]
 */

#define SYS_write       64
#define SYS_openat      56
#define SYS_close       57
#define SYS_read        63
#define SYS_lseek       62
#define SYS_exit        93
#define SYS_clock_nanosleep 115
#define SYS_newfstatat  79
#define SYS_readlink    78
#define SYS_mount       40
#define SYS_mknodat     33
#define SYS_umask       34

#define AT_FDCWD        (-100)
#define O_RDONLY        0
#define O_RDWR          2

#define S_IFCHR         0020000 /* asm-generic: 0o020000 == 8192 */

/* glibc-style makedev for the kernel's new_encode_dev layout. */
static unsigned long makedev_l(unsigned maj, unsigned min)
{
    return ((min & 0xffu) | ((maj & 0xfffu) << 8) | ((min & ~0xffu) << 12));
}

struct timespec { long tv_sec; long tv_nsec; };

/* Force a writable PT_LOAD: the freestanding binary otherwise has a single
 * RX segment (all-rodata), which exercises a different exec map path. */
volatile unsigned long g_writes = 0;

static long sys3(long n, long a, long b, long c)
{
    register long a0 __asm__("a0") = a;
    register long a1 __asm__("a1") = b;
    register long a2 __asm__("a2") = c;
    register long a7 __asm__("a7") = n;
    __asm__ volatile("ecall"
                     : "+r"(a0)
                     : "r"(a1), "r"(a2), "r"(a7)
                     : "memory");
    return a0;
}

static long sys4(long n, long a, long b, long c, long d)
{
    register long a0 __asm__("a0") = a;
    register long a1 __asm__("a1") = b;
    register long a2 __asm__("a2") = c;
    register long a3 __asm__("a3") = d;
    register long a7 __asm__("a7") = n;
    __asm__ volatile("ecall"
                     : "+r"(a0)
                     : "r"(a1), "r"(a2), "r"(a3), "r"(a7)
                     : "memory");
    return a0;
}

static long sys5(long n, long a, long b, long c, long d, long e)
{
    register long a0 __asm__("a0") = a;
    register long a1 __asm__("a1") = b;
    register long a2 __asm__("a2") = c;
    register long a3 __asm__("a3") = d;
    register long a4 __asm__("a4") = e;
    register long a7 __asm__("a7") = n;
    __asm__ volatile("ecall"
                     : "+r"(a0)
                     : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a7)
                     : "memory");
    return a0;
}

static unsigned slen(const char *s)
{
    unsigned n = 0;
    while (s[n])
        n++;
    return n;
}

static void puts_(const char *s)
{
    sys3(SYS_write, 1, (long)s, slen(s));
    g_writes++;
}

static void put_hex(unsigned long v)
{
    char buf[19];
    buf[0] = '0';
    buf[1] = 'x';
    for (int i = 0; i < 16; i++) {
        int nib = (v >> ((15 - i) * 4)) & 0xF;
        buf[2 + i] = nib < 10 ? '0' + nib : 'a' + (nib - 10);
    }
    buf[18] = '\n';
    sys3(SYS_write, 1, (long)buf, 19);
}

struct stat_lite {
    long pad[6];        /* st_dev..st_rdev (48 bytes before st_size) */
    long st_size;       /* offset 48 on riscv64 lp64 */
};

static void dump_file(const char *path)
{
    long fd = sys3(SYS_openat, AT_FDCWD, (long)path, 0 /*O_RDONLY*/);
    if (fd < 0) {
        puts_("INITTEST: open failed: ");
        puts_(path);
        puts_("\n");
        return;
    }
    char buf[512];
    long n;
    puts_("INITTEST: cmdline: ");
    while ((n = sys3(SYS_read, fd, (long)buf, sizeof(buf) - 1)) > 0) {
        buf[n] = 0;
        sys3(SYS_write, 1, (long)buf, n);
    }
    puts_("\n");
    sys3(SYS_close, fd, 0, 0);
}

static void check_node(const char *path, int use_lstat)
{
    /* riscv64 asm-generic struct stat: st_dev(8) st_ino(8) st_mode(u32@16)
     * st_nlink(u32@20) st_uid(24) st_gid(28) ... st_size(@48). */
    char st[160]; // full struct stat is 128B on riscv64 lp64
    for (volatile int i = 0; i < 160; i++)
        st[i] = 0;
    long r = sys4(SYS_newfstatat, AT_FDCWD, (long)path, (long)st,
                  use_lstat ? 0x100 /*AT_SYMLINK_NOFOLLOW*/ : 0);
    puts_(use_lstat ? "INITTEST: lstat " : "INITTEST: stat  ");
    puts_(path);
    if (r < 0) {
        puts_(" FAILED\n");
        return;
    }
    unsigned long mode = *(unsigned int *)(st + 16);
    puts_(" mode=");
    put_hex(mode);
    long size = *(long *)(st + 48);
    puts_(" size=");
    put_hex((unsigned long)size);
    puts_("\n");
}

static void check_readlink(const char *path)
{
    char buf[128];
    /* riscv64 has no readlink(2) — syscall 78 is readlinkat. */
    long n = sys4(78 /*readlinkat*/, AT_FDCWD, (long)path, (long)buf,
                  (long)sizeof(buf));
    puts_("INITTEST: readlink ");
    puts_(path);
    puts_(" -> ");
    if (n < 0) {
        puts_("FAILED\n");
        return;
    }
    buf[n] = 0;
    puts_(buf);
    puts_("\n");
}

/*
 * OH Phase 1b tmpfs-mknod probe: do exactly what OH's init does early in
 * the ramdisk — mount a fresh tmpfs over /dev and mknod the console-era
 * device nodes — then verify they behave (/dev/null read/write, random
 * source, kmsg sink). "DEVTEST:" lines are the boot-test markers.
 */
static void devtmpfs_mknod_probe(void)
{
    long r;

    /* Keep mknod modes exact (the kernel applies the umask). */
    sys3(SYS_umask, 0, 0, 0);

    r = sys5(SYS_mount, (long)"none", (long)"/dev", (long)"tmpfs", 0, 0);
    puts_(r == 0 ? "DEVTEST: mount tmpfs /dev ok\n"
                 : "DEVTEST: mount tmpfs /dev FAILED\n");
    if (r != 0)
        return;

    /* Diagnostic: /dev/console only exists in the boot devfs; after the
     * tmpfs overmount it must be GONE (ENOENT). If it is still there,
     * lookups are hitting devfs, not the tmpfs. */
    {
        long fd = sys4(SYS_openat, AT_FDCWD, (long)"/dev/console", O_RDONLY, 0);
        puts_(fd < 0 ? "DEVTEST: /dev/console gone after tmpfs overmount\n"
                     : "DEVTEST: /dev/console STILL VISIBLE (overmount not in effect)\n");
        if (fd >= 0)
            sys3(SYS_close, fd, 0, 0);
    }

    struct { const char *name; unsigned maj, min; int perm; } nodes[] = {
        { "/dev/null",    1,  3, 0666 },
        { "/dev/random",  1,  8, 0666 },
        { "/dev/urandom", 1,  9, 0666 },
        { "/dev/kmsg",    1, 11, 0600 },
    };
    for (unsigned i = 0; i < sizeof(nodes) / sizeof(nodes[0]); i++) {
        r = sys4(SYS_mknodat, AT_FDCWD, (long)nodes[i].name,
                 S_IFCHR | nodes[i].perm, (long)makedev_l(nodes[i].maj, nodes[i].min));
        if (r != 0) {
            puts_("DEVTEST: mknod failed: ");
            puts_(nodes[i].name);
            puts_("\n");
            return;
        }
    }
    puts_("DEVTEST: mknod null/random/urandom/kmsg ok\n");

    /* stat /dev/null: S_IFCHR + st_rdev == makedev(1,3) (offset 32, u64). */
    {
        char st[160];
        for (volatile int i = 0; i < 160; i++)
            st[i] = 0;
        r = sys4(SYS_newfstatat, AT_FDCWD, (long)"/dev/null", (long)st, 0);
        unsigned int mode = *(unsigned int *)(st + 16);
        unsigned long rdev = *(unsigned long *)(st + 32);
        if (r == 0 && (mode & 0170000) == S_IFCHR && rdev == makedev_l(1, 3)) {
            puts_("DEVTEST: stat /dev/null S_IFCHR rdev=1:3 ok\n");
        } else {
            puts_("DEVTEST: stat /dev/null mismatch (r=");
            put_hex(r);
            puts_(" mode=");
            put_hex(mode);
            puts_(" rdev=");
            put_hex(rdev);
            puts_(")\n");
        }
    }

    /* /dev/null: write discards, read EOFs. */
    {
        long fd = sys4(SYS_openat, AT_FDCWD, (long)"/dev/null", O_RDWR, 0);
        if (fd < 0) {
            puts_("DEVTEST: open /dev/null FAILED\n");
            return;
        }
        long w = sys3(SYS_write, fd, (long)"probe", 5);
        char buf[4];
        long rd = sys3(SYS_read, fd, (long)buf, sizeof(buf));
        puts_((w == 5 && rd == 0) ? "DEVTEST: /dev/null write+read ok\n"
                                  : "DEVTEST: /dev/null io mismatch\n");
        sys3(SYS_close, fd, 0, 0);
    }

    /* /dev/urandom: a read returns bytes. */
    {
        long fd = sys4(SYS_openat, AT_FDCWD, (long)"/dev/urandom", O_RDONLY, 0);
        if (fd < 0) {
            puts_("DEVTEST: open /dev/urandom FAILED\n");
            return;
        }
        unsigned char b[8] = { 0 };
        long rd = sys3(SYS_read, fd, (long)b, sizeof(b));
        puts_((rd == 8) ? "DEVTEST: /dev/urandom read ok\n"
                        : "DEVTEST: /dev/urandom read mismatch\n");
        sys3(SYS_close, fd, 0, 0);
    }

    /* /dev/kmsg: a write is swallowed by the kernel log. */
    {
        long fd = sys4(SYS_openat, AT_FDCWD, (long)"/dev/kmsg", O_RDWR, 0);
        if (fd < 0) {
            puts_("DEVTEST: open /dev/kmsg FAILED\n");
            return;
        }
        long w = sys3(SYS_write, fd, (long)"<6>DEVTEST: hello via kmsg\n", 26);
        puts_((w == 26) ? "DEVTEST: /dev/kmsg write ok\n"
                        : "DEVTEST: /dev/kmsg write mismatch\n");
        sys3(SYS_close, fd, 0, 0);
    }
}

#ifdef HELLO
void _start(void)
{
    /* The freestanding binary has no CRT: initialize gp ourselves, or
     * linker-relaxed gp-relative data accesses (g_writes) jump through
     * a garbage pointer. */
    __asm__ volatile(
        ".option push\n"
        ".option norelax\n"
        "la gp, __global_pointer$\n"
        ".option pop\n"
        :
        :
        : "gp");
    puts_("HELLO-FROM-INITRD-BIN\n");
    sys3(SYS_exit, 0, 0, 0);
    __builtin_unreachable();
}
#else
void _start(void)
{
    /* The freestanding binary has no CRT: initialize gp ourselves, or
     * linker-relaxed gp-relative data accesses (g_writes) jump through
     * a garbage pointer. */
    __asm__ volatile(
        ".option push\n"
        ".option norelax\n"
        "la gp, __global_pointer$\n"
        ".option pop\n"
        :
        :
        : "gp");
    puts_("INITTEST: /init from initrd is running (PID 1)\n");
    dump_file("/proc/cmdline");
    check_node("/bin/lnk", 1);   /* lstat a symlink FIRST (isolation) */
    check_node("/bin/hello", 0);
    check_node("/bin/hard", 0);
    check_node("/etc", 0);
    puts_("\n");
    check_readlink("/bin/lnk");

    devtmpfs_mknod_probe();

    int beat = 0;
    struct timespec ts = { 2, 0 };
    while (1) {
        sys4(SYS_clock_nanosleep, 1 /*CLOCK_MONOTONIC*/, 0, (long)&ts, 0);
        beat++;
        puts_(beat % 5 == 0 ? "INITTEST: still alive (10s)\n"
                            : "INITTEST: beat\n");
    }
}
#endif
