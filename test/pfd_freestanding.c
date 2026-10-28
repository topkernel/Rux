/* pfd_freestanding.c — /proc/self/fd magic-link + O_TMPFILE semantics probe
 * (no libc; the host toolchain's static glibc carries RVV instructions the
 * guest CPU state cannot execute). Raw riscv64 syscalls only.
 *
 * Build:
 *   riscv64-linux-gnu-gcc -static -ffreestanding -nostdlib \
 *       -march=rv64gc -o pfd_fs pfd_freestanding.c
 */
typedef unsigned long u64;
typedef long i64;
typedef unsigned short u16;
typedef unsigned int u32;
typedef unsigned char u8;

static inline i64 sys4(i64 n, i64 a, i64 b, i64 c, i64 d)
{
    register i64 a0 asm("a0") = a;
    register i64 a1 asm("a1") = b;
    register i64 a2 asm("a2") = c;
    register i64 a3 asm("a3") = d;
    register i64 a7 asm("a7") = n;
    asm volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a3), "r"(a7) : "memory");
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

#define AT_FDCWD ((i64)-100)
#define __NR_openat 56
#define __NR_close 57
#define __NR_getdents64 61
#define __NR_read 63
#define __NR_write 64
#define __NR_readlinkat 78
#define __NR_newfstatat 79
#define __NR_fstat 80
#define __NR_unlinkat 35
#define __NR_linkat 37
#define __NR_fchmodat 53
#define __NR_mkdirat 34

#define O_RDONLY 0
#define O_RDWR 2
#define O_DIRECTORY 0200000
#define O_TMPFILE 020200000 /* __O_TMPFILE | O_DIRECTORY */
#define AT_SYMLINK_FOLLOW 0x400

static i64 openat_(const char *p, i64 flags, i64 mode)
{ return sys4(__NR_openat, AT_FDCWD, (i64)p, flags, mode); }
static i64 close_(i64 fd) { return sys4(__NR_close, fd, 0, 0, 0); }
static i64 write_(i64 fd, const void *b, u64 n)
{ return sys4(__NR_write, fd, (i64)b, (i64)n, 0); }
static i64 unlink_(const char *p)
{ return sys4(__NR_unlinkat, AT_FDCWD, (i64)p, 0, 0); }

struct mini_stat { u64 pad[6]; u64 st_size; u64 pad2[11]; };

static void puts_(const char *s)
{
    u64 n = 0;
    while (s[n]) n++;
    write_(1, s, n);
    write_(1, "\n", 1);
}
static void putdec(i64 v)
{
    char b[24];
    int n = 0;
    if (v < 0) { write_(1, "-", 1); v = -v; }
    if (v == 0) b[n++] = '0';
    while (v > 0) { b[n++] = '0' + (v % 10); v /= 10; }
    char r[24];
    for (int i = 0; i < n; i++) r[i] = b[n - 1 - i];
    write_(1, r, n);
    write_(1, "\n", 1);
}
static void report(const char *what, i64 rc)
{
    puts_(what);
    puts_(" rc=");
    putdec(rc);
    if (rc < 0) { puts_(" errno="); putdec(-rc); }
    puts_("");
}

static char readlink_buf[256];
static char wbuf[1024];

struct linux_dirent64 {
    u64 d_ino;
    i64 d_off;
    u16 d_reclen;
    u8 d_type;
    char d_name[];
};

int main(void)
{
    puts_("PFD-START");

    /* --- part 1: /proc/self/fd/N magic link on a NORMAL file --- */
    unlink_("/tmp/pfd_t");
    i64 fd = openat_("/tmp/pfd_t", O_RDWR | 0100 /*O_CREAT*/, 0644);
    report("creat /tmp/pfd_t", fd);
    i64 r = sys5(__NR_readlinkat, AT_FDCWD, (i64)"/proc/self/fd/3",
                 (i64)readlink_buf, sizeof(readlink_buf) - 1, 0);
    report("readlinkat /proc/self/fd/3", r);
    if (r > 0) {
        readlink_buf[r] = 0;
        puts_("  target: ");
        puts_(readlink_buf);
        puts_("");
    }
    r = sys5(__NR_fchmodat, AT_FDCWD, (i64)"/proc/self/fd/3", 0755, 0, 0);
    report("fchmodat /proc/self/fd/3", r);
    r = sys5(__NR_linkat, AT_FDCWD, (i64)"/proc/self/fd/3",
             AT_FDCWD, (i64)"/tmp/pfd_l", AT_SYMLINK_FOLLOW);
    report("linkat /proc/self/fd/3 -> /tmp/pfd_l", r);
    struct mini_stat st;
    r = sys5(__NR_newfstatat, AT_FDCWD, (i64)"/tmp/pfd_l", (i64)&st, 0, 0);
    report("stat /tmp/pfd_l", r);

    /* --- part 2: O_TMPFILE --- */
    i64 tfd = openat_("/tmp", O_TMPFILE | O_RDWR, 0600);
    report("openat /tmp O_TMPFILE|O_RDWR", tfd);
    if (tfd >= 0) {
        for (int i = 0; i < 4; i++) {
            i64 w = write_(tfd, wbuf, sizeof(wbuf));
            if (w != (i64)sizeof(wbuf)) { report("  write", w); break; }
        }
        r = sys4(__NR_fstat, tfd, (i64)&st, 0, 0);
        report("  fstat", r);
        puts_("  size=");
        putdec((i64)st.st_size);
        puts_("");
        /* dir must be empty of NEW entries: dump getdents of /tmp tail */
        i64 dfd = openat_("/tmp", O_DIRECTORY | O_RDONLY, 0);
        report("  opendir /tmp", dfd);
        if (dfd >= 0) {
            static char dbuf[4096];
            i64 n = sys4(__NR_getdents64, dfd, (i64)dbuf, sizeof(dbuf), 0);
            report("  getdents64", n);
            if (n > 0) {
                i64 off = 0;
                int cnt = 0;
                while (off < n) {
                    struct linux_dirent64 *d = (struct linux_dirent64 *)(dbuf + off);
                    puts_("   ");
                    puts_(d->d_name);
                    puts_("");
                    cnt++;
                    off += d->d_reclen;
                }
                puts_("  entries=");
                putdec(cnt);
                puts_("");
            }
            close_(dfd);
        }
        /* readlink the tmpfile fd */
        char p32[32];
        const char *pf = p32;
        /* build "/proc/self/fd/N" by hand */
        {
            char *s = p32;
            const char *pre = "/proc/self/fd/";
            while (*pre) *s++ = *pre++;
            i64 v = tfd;
            char tb[8]; int tn = 0;
            if (v == 0) tb[tn++] = '0';
            while (v > 0) { tb[tn++] = '0' + (v % 10); v /= 10; }
            while (tn > 0) *s++ = tb[--tn];
            *s = 0;
        }
        r = sys5(__NR_readlinkat, AT_FDCWD, (i64)pf,
                 (i64)readlink_buf, sizeof(readlink_buf) - 1, 0);
        report("  readlinkat tmpfile-fd", r);
        if (r > 0) {
            readlink_buf[r] = 0;
            puts_("  target: ");
            puts_(readlink_buf);
            puts_("");
        }
        r = sys5(__NR_linkat, AT_FDCWD, (i64)pf,
                 AT_FDCWD, (i64)"/tmp/pfd_materia", AT_SYMLINK_FOLLOW);
        report("  linkat tmpfile -> /tmp/pfd_materia", r);
        struct mini_stat st2;
        r = sys5(__NR_newfstatat, AT_FDCWD, (i64)"/tmp/pfd_materia",
                 (i64)&st2, 0, 0);
        report("  stat /tmp/pfd_materia", r);
        close_(tfd);
    }

    /* --- part 3: open14 test03 shape — nested dirs + tmpfile link --- */
    {
        static char cwd2[256];
        r = sys4(34 /*mkdirat*/, AT_FDCWD, (i64)"/tmp/pfd_d0", 0700, 0);
        report("mkdir /tmp/pfd_d0", r);
        r = sys5(79, AT_FDCWD, (i64)"/tmp", (i64)&st, 0, 0);
        report(" fstatat /tmp", r);
        puts_(" /tmp mode=");
        putdec((i64)(st.pad[2] & 0xFFFF));
        puts_("");
        r = sys5(79, AT_FDCWD, (i64)"/tmp/pfd_d0", (i64)&st, 0, 0);
        report(" fstatat /tmp/pfd_d0", r);
        puts_(" pfd_d0 mode=");
        putdec((i64)(st.pad[2] & 0xFFFF));
        puts_("");
        r = sys4(49 /*chdir*/, (i64)"/tmp/pfd_d0", 0, 0, 0);
        report("chdir /tmp/pfd_d0", r);
        i64 fd0 = openat_(".", O_TMPFILE | O_RDWR, 0644);
        report(" tmpfile in d0", fd0);
        if (fd0 >= 0) {
            write_(fd0, wbuf, sizeof(wbuf));
            char lp[24];
            const char *lpp = lp;
            {
                char *s = lp;
                const char *pre = "/proc/self/fd/";
                while (*pre) *s++ = *pre++;
                i64 v = fd0;
                char tb[8]; int tn = 0;
                while (v > 0) { tb[tn++] = '0' + (v % 10); v /= 10; }
                while (tn > 0) *s++ = tb[--tn];
                *s = 0;
            }
            r = sys5(__NR_linkat, AT_FDCWD, (i64)lpp,
                     AT_FDCWD, (i64)"mat0", AT_SYMLINK_FOLLOW);
            report(" linkat -> mat0", r);
            close_(fd0);
        }
        r = sys4(34, AT_FDCWD, (i64)"d1", 0700, 0);
        report("mkdir d1 (relative)", r);
        r = sys5(79 /*newfstatat*/, AT_FDCWD, (i64)"d1", (i64)&st, 0, 0);
        report(" fstatat d1", r);
        puts_(" d1 mode_bits=");
        putdec((i64)((st.pad[2] >> 32) & 0xF000)); /* st_mode lives at offset 16 */
        puts_("");
        r = sys4(49, (i64)"d1", 0, 0, 0);
        report(" chdir d1", r);
        /* print cwd via /proc/self/cwd readlink */
        r = sys5(__NR_readlinkat, AT_FDCWD, (i64)"/proc/self/cwd",
                 (i64)cwd2, sizeof(cwd2) - 1, 0);
        if (r > 0) {
            cwd2[r] = 0;
            puts_(" cwd=");
            puts_(cwd2);
            puts_("");
        }
        /* leave things tidy: back out and remove */
        sys4(49, (i64)"/tmp", 0, 0, 0);
        sys4(__NR_unlinkat, AT_FDCWD, (i64)"/tmp/pfd_d0/mat0", 0, 0);
        sys4(__NR_unlinkat, AT_FDCWD, (i64)"/tmp/pfd_d0/d1", 0x200, 0);
        sys4(__NR_unlinkat, AT_FDCWD, (i64)"/tmp/pfd_d0", 0x200, 0);
    }

    unlink_("/tmp/pfd_t");
    unlink_("/tmp/pfd_l");
    unlink_("/tmp/pfd_materia");
    puts_("PFD-DONE");
    sys4(93, 0, 0, 0, 0); /* exit(0) */
    return 0;
}
