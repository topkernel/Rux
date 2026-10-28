/* probe6s.c — single-writer pwrite/pread coherence probe, raw-syscall
 * edition (no libc; safe ISA profile for the host toolchain quirk).
 *
 * Build: riscv64-linux-gnu-gcc -nostdlib -static -march=rv64imac -mabi=lp64 \
 *          -O2 -o probe6s probe6s.c
 *
 * One process, one fd. Ten 2048-byte chunks at offsets that SPAN block
 * boundaries (like the ftest04/zzprobe4 interleave); each chunk is
 * pwritten and immediately pread back. A ZERO/WRONG readback with no
 * second writer anywhere is the single-writer minimization of the
 * getblk_zero shared-block wipe family.
 *   P6S c=<i> off=<hex> wr=<rc> IMM=<ok|ZERO|WRONG(first=XX)>
 *   P6S SWEEP bad=<n>/10
 *   P6S-DONE
 */

#define __NR_openat    56
#define __NR_close     57
#define __NR_write     64
#define __NR_pread64   67
#define __NR_pwrite64  68
#define __NR_ftruncate 46
#define __NR_exit      93
#define __NR_exit_group 94
#define __NR_unlinkat  35

typedef long i64;
typedef unsigned long u64;

static i64 sys(i64 n, i64 a, i64 b, i64 c, i64 d, i64 e, i64 f)
{
    register i64 a0 __asm__("a0") = a;
    register i64 a1 __asm__("a1") = b;
    register i64 a2 __asm__("a2") = c;
    register i64 a3 __asm__("a3") = d;
    register i64 a4 __asm__("a4") = e;
    register i64 a5 __asm__("a5") = f;
    register i64 nr __asm__("a7") = n;
    __asm__ volatile("ecall"
                     : "+r"(a0)
                     : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a5), "r"(nr)
                     : "memory");
    return a0;
}

static void putstr(const char *s)
{
    i64 n = 0;
    while (s[n]) n++;
    sys(__NR_write, 2, (i64)s, n, 0, 0, 0);
}

/* Freestanding shims: -nostdlib still lets gcc emit memset/memcpy. */
void *memset(void *d, int c, unsigned long n)
{
    unsigned char *p = d;
    for (unsigned long i = 0; i < n; i++) p[i] = (unsigned char)c;
    return d;
}

void *memcpy(void *d, const void *s, unsigned long n)
{
    unsigned char *p = d;
    const unsigned char *q = s;
    for (unsigned long i = 0; i < n; i++) p[i] = q[i];
    return d;
}

static void putu(u64 v)
{
    char b[24];
    int n = 0;
    if (!v) b[n++] = '0';
    while (v) { b[n++] = '0' + (v % 10); v /= 10; }
    char o[24];
    for (int i = 0; i < n; i++) o[i] = b[n - 1 - i];
    sys(__NR_write, 2, (i64)o, n, 0, 0, 0);
}

static void puthex(u64 v)
{
    char o[19];
    o[0] = '0'; o[1] = 'x';
    static const char h[] = "0123456789abcdef";
    for (int i = 0; i < 16; i++) o[2 + i] = h[(v >> (60 - 4 * i)) & 0xf];
    sys(__NR_write, 2, (i64)o, 18, 0, 0, 0);
}

#define CSIZE 2048
#define NCHUNK 10

static unsigned char wbuf[CSIZE], rbuf[CSIZE];

static const char path[] = "/tmp/probe6s.dat";

void _start(void)
{
    i64 fd = sys(__NR_openat, -100, (i64)path, 0x42 /*O_RDWR|O_CREAT*/, 0777, 0, 0);
    if (fd < 0) {
        putstr("P6S open-failed\n"); sys(__NR_exit_group, 1, 0, 0, 0, 0, 0);
    }
    sys(__NR_ftruncate, fd, (NCHUNK * CSIZE) + 4096, 0, 0, 0, 0);

    int bad = 0;
    for (int c = 0; c < NCHUNK; c++) {
        unsigned char pat = 1 + c * 7;
        for (int i = 0; i < CSIZE; i++) wbuf[i] = pat;
        /* Mid-block offsets so every chunk SPANS a block boundary
         * (offset+2048 crosses 4096 for off%4096 in [2048,4096)). */
        u64 off = (u64)c * CSIZE + 3800;

        i64 w = sys(__NR_pwrite64, fd, (i64)wbuf, CSIZE, (i64)off, 0, 0);
        i64 r = sys(__NR_pread64, fd, (i64)rbuf, CSIZE, (i64)off, 0, 0);

        putstr("P6S c="); putu(c);
        putstr(" off="); puthex(off);
        putstr(" wr="); putu(w);
        if (r != w) { putstr(" r="); putu(r); }
        if (r == CSIZE) {
            int z = 1, m = 1;
            for (int i = 0; i < CSIZE; i++) {
                if (rbuf[i]) z = 0;
                if (rbuf[i] != pat) m = 0;
            }
            if (m) putstr(" IMM=ok");
            else if (z) { putstr(" IMM=ZERO"); bad++; }
            else { putstr(" IMM=WRONG(first="); puthex(rbuf[0]); putstr(")"); bad++; }
        } else {
            putstr(" IMM=SHORT"); bad++;
        }
        putstr("\n");
    }
    putstr("P6S SWEEP bad="); putu(bad); putstr("/10\n");
    putstr("P6S-DONE\n");
    sys(__NR_close, fd, 0, 0, 0, 0, 0);
    sys(__NR_unlinkat, -100, (i64)path, 0, 0, 0, 0);
    sys(__NR_exit_group, 0, 0, 0, 0, 0, 0);
}
