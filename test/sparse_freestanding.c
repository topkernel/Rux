/* sparse_freestanding.c — openat02 O_LARGEFILE repro without libc
 * (the host toolchain's static glibc carries RVV instructions the guest
 * CPU state cannot execute). Raw riscv64 syscalls only.
 *
 * Prints statfs free-block deltas around: seek to 4.1 GiB + write 7
 * bytes + read-back verify. Build:
 *   riscv64-linux-gnu-gcc -static -ffreestanding -nostdlib \
 *       -march=rv64gc -o sparse_fs sparse_freestanding.c
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

static i64 openat(const char *p, i64 flags)
{ return sys3(56, -100, (i64)p, flags); }
static i64 lseek(i64 fd, i64 off, i64 whence)
{ return sys3(62, fd, off, whence); }
static i64 write(i64 fd, const void *b, u64 n)
{ return sys3(64, fd, (i64)b, (i64)n); }
static i64 readf(i64 fd, void *b, u64 n)
{ return sys3(63, fd, (i64)b, (i64)n); }
static i64 close(i64 fd) { return sys3(57, fd, 0, 0); }
/* riscv64 generic table: 35 = unlinkat(dirfd, path, flags) */
static i64 unlink(const char *p) { return sys3(35, -100, (i64)p, 0); }

struct mini_statfs { u64 pad1[4]; u64 f_bfree; u64 f_bavail; u64 pad2[8]; };
static i64 statfs_tmp(void)
{
    struct mini_statfs st;
    i64 r = sys3(43, (i64)"/tmp", (i64)&st, sizeof(st));
    if (r < 0) return r;
    return (i64)st.f_bfree;
}

struct mini_stat { u64 pad[7]; u64 st_size; u64 pad2[11]; };
static i64 fstat_size(i64 fd, u64 *out)
{
    struct mini_stat st;
    i64 r = sys3(80, fd, (i64)&st, sizeof(st));
    if (r == 0) *out = st.st_size;
    return r;
}

static void puts_(const char *s)
{
    u64 n = 0;
    while (s[n]) n++;
    write(1, s, n);
    write(1, "\n", 1);
}
static void putdec(i64 v)
{
    char b[24];
    int n = 0;
    if (v < 0) { write(1, "-", 1); v = -v; }
    if (v == 0) b[n++] = '0';
    while (v > 0) { b[n++] = '0' + (v % 10); v /= 10; }
    char r[24];
    for (int i = 0; i < n; i++) r[i] = b[n - 1 - i];
    write(1, r, n);
    write(1, "\n", 1);
}
static void puthex(u64 v)
{
    char b[17];
    for (int i = 15; i >= 0; i--) {
        u64 d = (v >> (i * 4)) & 0xF;
        b[15 - i] = d < 10 ? '0' + d : 'a' + d - 10;
    }
    b[16] = 0;
    puts_(b);
}

void _start(void)
{
    puts_("SPARSE-PROBE-START");
    puts_("free_before:");
    putdec(statfs_tmp());

    i64 fd = openat("/tmp/sparse_test", 0x41 /*O_WRONLY|O_CREAT*/);
    puts_("fd:");
    putdec(fd);
    if (fd < 0) goto out;

    i64 target = 44; target = target * 1024 * 1024 * 1024 / 10; /* 4.4 GiB */
    puts_("lseek:");
    putdec(lseek(fd, target, 0));

    i64 n = write(fd, "abcdefg", 7);
    puts_("write_n:");
    putdec(n);
    puts_("free_after_write:");
    putdec(statfs_tmp());

    u64 sz = 0;
    fstat_size(fd, &sz);
    puts_("size:");
    putdec((i64)sz);
    puts_("size_want:");
    putdec(target + 7);

    /* read-back via a second fd */
    i64 rfd = openat("/tmp/sparse_test", 0 /*O_RDONLY*/);
    if (rfd >= 0) {
        char buf[40];
        for (int i = 0; i < 40; i++) buf[i] = (char)0xAA;
        lseek(rfd, target - 16, 0);
        i64 r = readf(rfd, buf, 40);
        puts_("read_n:");
        putdec(r);
        puts_("read_hex:");
        for (int i = 0; i < (r > 0 ? (int)r : 0); i++) {
            char h[3] = { '0' + ((buf[i] >> 4) & 0xF), '0' + (buf[i] & 0xF), 0 };
            if (h[0] > '9') h[0] += 'a' - '9' - 1;
            if (h[1] > '9') h[1] += 'a' - '9' - 1;
            write(1, h, 2);
        }
        write(1, "\n", 1);
        close(rfd);
    }
    close(fd);
    unlink("/tmp/sparse_test");
    puts_("free_after_unlink:");
    putdec(statfs_tmp());
out:
    puts_("SPARSE-PROBE-DONE");
    sys3(93, 0, 0, 0);
    __builtin_unreachable();
}
