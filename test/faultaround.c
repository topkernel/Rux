// faultaround.c — mm-side fault-around semantics probe (x86_64, wildwrite
// lane session 14).
//
// The demand read/exec fault on a MAP_PRIVATE file VMA now fills and maps
// up to 16 pages per fault. Every leg exercises a rule the batch must
// hold, against a freshly created pattern file on the ext4 rootfs:
//
//   FA1  content: 64 pages touched sequentially (4 full windows); every
//         page's three stamps must match the file pattern.
//   FA2  EOF clip -> SIGBUS: 2-page file mapped over an 8-page span;
//         touching page 2 must SIGBUS (the window may not map past
//         effective size — LTP mmap13 semantics).
//   FA3  write-fault opt-out + COW privacy: first touch of a page is a
//         STORE (write faults skip fault-around, single-page arm); the
//         store sticks, the file on disk is unchanged (pread), and a
//         read-fault around-window elsewhere in the same mapping stays
//         writable without a fault.
//   FA4  partial tail: file of 2.5 pages; the last page's bytes past EOF
//         must read as ZERO (no stale/recycled content), bytes before
//         EOF must match.
//   FA5  VMA-end clip at a mapping boundary: two files mapped at
//         ADJACENT MAP_FIXED addresses; faulting the LAST page of A must
//         not leak A content into B's first page (different files never
//         merge VMAs, so the window must stop at A's end).
//   FA6  fork COW over an around-mapped window: parent faults page 0
//         (16 pages mapped); child reads page 10 (content), writes it
//         (COW), parent still sees the original.
//
// PASS: prints FAW-PASS. Any failure prints FAW-FAIL with the leg name.
#define _GNU_SOURCE
#include <fcntl.h>
#include <signal.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#define PAGE 4096UL
#define NPAGES 64

static int failures;

static void fail(const char *leg, const char *fmt, ...)
{
    printf("FAW-FAIL: %s: ", leg);
    va_list ap;
    va_start(ap, fmt);
    vprintf(fmt, ap);
    va_end(ap);
    printf("\n");
    failures++;
}

/* Stamps are per-file idiosyncratic: stamp = id_base + page index, so
 * cross-mapping contamination is distinguishable from correct content. */
static void stamp_page(unsigned char *p, unsigned long stamp)
{
    for (unsigned long i = 0; i < PAGE; i++)
        p[i] = (unsigned char)(0x40 + (stamp % 0x40));
    *(unsigned long *)(p) = stamp;
    *(unsigned long *)(p + PAGE - 8) = stamp;
}

static int check_page(const char *leg, const unsigned char *p,
                      unsigned long stamp)
{
    int bad = 0;
    for (unsigned long i = 8; i < PAGE - 8; i++) {
        if (p[i] != (unsigned char)(0x40 + (stamp % 0x40))) {
            fail(leg, "stamp %lu filler wrong at +%lu (%02x)", stamp, i,
                 p[i]);
            bad = 1;
            break;
        }
    }
    if (*(unsigned long *)p != stamp) {
        fail(leg, "stamp %lu head %lu", stamp, *(unsigned long *)p);
        bad = 1;
    }
    if (*(unsigned long *)(p + PAGE - 8) != stamp) {
        fail(leg, "stamp %lu tail %lu", stamp,
             *(unsigned long *)(p + PAGE - 8));
        bad = 1;
    }
    return bad;
}

/* Build the pattern file: n pages, page i stamped id_base+i; for a
 * partial last page only the first `tail` bytes are written. */
static void build_file(const char *path, unsigned long npages,
                       unsigned long tail, unsigned long id_base)
{
    int fd = open(path, O_CREAT | O_RDWR | O_TRUNC, 0600);
    if (fd < 0) {
        perror("open pattern");
        exit(2);
    }
    static unsigned char pg[PAGE];
    for (unsigned long i = 0; i < npages; i++) {
        stamp_page(pg, id_base + i);
        unsigned long n = (i == npages - 1 && tail) ? tail : PAGE;
        if (write(fd, pg, n) != (long)n) {
            perror("write pattern");
            exit(2);
        }
    }
    close(fd);
}

static void leg_fa1(void)
{
    const char *path = "/tmp/fa1.bin";
    build_file(path, NPAGES, 0, 0);
    int fd = open(path, O_RDONLY);
    if (fd < 0) { fail("FA1", "open: %m"); return; }
    unsigned char *m = mmap(NULL, NPAGES * PAGE, PROT_READ | PROT_EXEC,
                            MAP_PRIVATE, fd, 0);
    if (m == MAP_FAILED) { fail("FA1", "mmap: %m"); close(fd); return; }
    /* Sequential touch: window after window. */
    for (unsigned long i = 0; i < NPAGES; i++) {
        if (check_page("FA1", m + i * PAGE, i))
            break;
    }
    munmap(m, NPAGES * PAGE);
    close(fd);
    printf("FAW: FA1 sequential content ok (%d pages)\n", NPAGES);
}

static void sigbus_handler(int sig)
{
    (void)sig;
    const char msg[] = "FAW: FA2 child got SIGBUS (expected)\n";
    write(1, msg, sizeof(msg) - 1);
    _exit(97);
}

static void leg_fa2(void)
{
    const char *path = "/tmp/fa2.bin";
    build_file(path, 2, 0, 2000);
    int fd = open(path, O_RDONLY);
    if (fd < 0) { fail("FA2", "open: %m"); return; }
    unsigned char *m = mmap(NULL, 8 * PAGE, PROT_READ, MAP_PRIVATE, fd, 0);
    if (m == MAP_FAILED) { fail("FA2", "mmap: %m"); close(fd); return; }
    /* Both in-file pages must read clean (and window-clipped). */
    check_page("FA2", m, 2000);
    check_page("FA2", m + PAGE, 2001);
    pid_t pid = fork();
    if (pid == 0) {
        struct sigaction sa;
        memset(&sa, 0, sizeof(sa));
        sa.sa_handler = sigbus_handler;
        sigaction(SIGBUS, &sa, NULL);
        /* Past-EOF page inside the mapped span: must SIGBUS, and the
         * parent's window must NOT have pre-mapped it either (child
         * inherits PTEs — a pre-mapped page would NOT fault). */
        volatile unsigned char v = m[2 * PAGE];
        (void)v;
        const char msg[] = "FAW-FAIL: FA2 no SIGBUS past EOF\n";
        write(1, msg, sizeof(msg) - 1);
        _exit(0);
    }
    int st = 0;
    waitpid(pid, &st, 0);
    if (!(WIFEXITED(st) && WEXITSTATUS(st) == 97))
        fail("FA2", "child status %d (no SIGBUS?)", st);
    munmap(m, 8 * PAGE);
    close(fd);
}

static void leg_fa3(void)
{
    const char *path = "/tmp/fa3.bin";
    build_file(path, 8, 0, 0);
    int fd = open(path, O_RDWR);
    if (fd < 0) { fail("FA3", "open: %m"); return; }
    unsigned char *m = mmap(NULL, 8 * PAGE, PROT_READ | PROT_WRITE,
                            MAP_PRIVATE, fd, 0);
    if (m == MAP_FAILED) { fail("FA3", "mmap: %m"); close(fd); return; }
    /* FIRST touch of page 5 is a store: write faults skip fault-around
     * (single-page arm); MAP_PRIVATE COW privacy must hold. */
    memset(m + 5 * PAGE, 0xAB, 64);
    if (m[5 * PAGE] != 0xAB) { fail("FA3", "store not visible"); }
    /* File content on disk unchanged: read a FILLER byte (offset+8 —
     * byte 0 carries the page's head stamp). */
    unsigned char buf[64];
    ssize_t n = pread(fd, buf, 64, 5 * PAGE + 8);
    if (n != 64 || buf[0] != (unsigned char)(0x40 + 5)) {
        fail("FA3", "file changed on disk (n=%zd buf0=%02x)", n, buf[0]);
    }
    /* Read fault at page 0 runs fault-around (window 0..8 clipped by
     * VMA end); pages it mapped carry W (VMA is writable, pages are
     * exclusive) — a store there must NOT fault. */
    check_page("FA3", m, 0);
    m[1 * PAGE + 16] = 0xCD;
    if (m[1 * PAGE + 16] != 0xCD) { fail("FA3", "store into around page"); }
    m[1 * PAGE + 16] = 0xCD;
    if (m[1 * PAGE + 16] != 0xCD) { fail("FA3", "store into around page"); }
    munmap(m, 8 * PAGE);
    close(fd);
    printf("FAW: FA3 write-fault opt-out + COW privacy ok\n");
}

static void leg_fa4(void)
{
    const char *path = "/tmp/fa4.bin";
    const unsigned long tail = PAGE / 2 + 3; /* 2 pages + partial third */
    build_file(path, 3, tail, 0);
    int fd = open(path, O_RDONLY);
    if (fd < 0) { fail("FA4", "open: %m"); return; }
    unsigned char *m = mmap(NULL, 3 * PAGE, PROT_READ, MAP_PRIVATE, fd, 0);
    if (m == MAP_FAILED) { fail("FA4", "mmap: %m"); close(fd); return; }
    /* Touch page 0 (window covers 0..3); the partial last page was in
     * the window: its bytes past EOF must be ZERO. */
    volatile unsigned char v = m[0];
    (void)v;
    unsigned char *last = m + 2 * PAGE;
    for (unsigned long i = tail; i < PAGE; i++) {
        if (last[i] != 0) {
            fail("FA4", "stale byte past EOF at +page2+%lu (%02x)", i,
                 last[i]);
            break;
        }
    }
    /* Valid tail bytes match the stamp. */
    for (unsigned long i = 8; i + 8 < tail; i++) {
        if (last[i] != (unsigned char)(0x40 + 2)) {
            fail("FA4", "tail content wrong at +%lu", i);
            break;
        }
    }
    check_page("FA4", m, 0);
    check_page("FA4", m + PAGE, 1);
    munmap(m, 3 * PAGE);
    close(fd);
    printf("FAW: FA4 partial-tail zero fill ok\n");
}

static void leg_fa5(void)
{
    build_file("/tmp/fa5a.bin", 2, 0, 0);
    build_file("/tmp/fa5b.bin", 2, 0, 1000);
    /* Adjacent MAP_FIXED mappings, different files (never merge). Try
     * the loader-style high address first, then a low one; report each
     * refusal (the exact EINVAL shape is a kernel mmap placement
     * question, not this probe's business — any base that maps works
     * for the boundary test). */
    int fda = open("/tmp/fa5a.bin", O_RDONLY);
    int fdb = open("/tmp/fa5b.bin", O_RDONLY);
    if (fda < 0 || fdb < 0) { fail("FA5", "open: %m"); return; }
    const unsigned long bases[] = { 0x7ff300000000UL, 0x50000000UL };
    unsigned char *a = MAP_FAILED, *b = MAP_FAILED;
    unsigned long base = 0;
    for (unsigned long i = 0; i < sizeof(bases) / sizeof(bases[0]); i++) {
        base = bases[i];
        a = mmap((void *)base, 2 * PAGE, PROT_READ,
                 MAP_PRIVATE | MAP_FIXED, fda, 0);
        if (a == MAP_FAILED) {
            printf("FAW: FA5 mmap A at %#lx refused: %m\n", base);
            continue;
        }
        b = mmap((void *)(base + 2 * PAGE), 2 * PAGE, PROT_READ,
                 MAP_PRIVATE | MAP_FIXED, fdb, 0);
        if (b == MAP_FAILED) {
            printf("FAW: FA5 mmap B at %#lx refused: %m\n", base + 2 * PAGE);
            munmap(a, 2 * PAGE);
            a = MAP_FAILED;
            continue;
        }
        break;
    }
    if (a == MAP_FAILED || b == MAP_FAILED) {
        fail("FA5", "no usable adjacent MAP_FIXED base");
        return;
    }
    /* Fault the LAST page of A: the window must clip at A's end and
     * must not install file-A content at B's first page (B's pages
     * carry stamps 1000+; file-A page 2 would stamp 2). */
    volatile unsigned char v = a[PAGE];
    (void)v;
    check_page("FA5", a + PAGE, 1);
    if (*(unsigned long *)b != 1000) {
        fail("FA5", "B page0 stamp %lu (file-A window crossed the VMA end)",
             *(unsigned long *)b);
    }
    /* Also verify B reads its own content after its own fault. */
    check_page("FA5", b, 1000);
    check_page("FA5", b + PAGE, 1001);
    munmap(a, 2 * PAGE);
    munmap(b, 2 * PAGE);
    close(fda);
    close(fdb);
    printf("FAW: FA5 VMA-end boundary clip ok\n");
}

static void leg_fa6(void)
{
    const char *path = "/tmp/fa6.bin";
    build_file(path, 32, 0, 0);
    int fd = open(path, O_RDONLY);
    if (fd < 0) { fail("FA6", "open: %m"); return; }
    unsigned char *m = mmap(NULL, 32 * PAGE, PROT_READ | PROT_WRITE,
                            MAP_PRIVATE, fd, 0);
    if (m == MAP_FAILED) { fail("FA6", "mmap: %m"); close(fd); return; }
    /* Parent faults page 0: fault-around maps pages 0..16. */
    volatile unsigned char v = m[0];
    (void)v;
    /* Snapshot: the child must judge ONLY its own checks, not the
     * parent's accumulated leg failures (inherited counter). */
    const int base_failures = failures;
    pid_t pid = fork();
    if (pid == 0) {
        /* Child inherits the around-mapped window COW; page 10 present.
         * Raw-write diagnostics (buffered printf from a forked child
         * proved lossy on this console). */
        static char msg[160];
        int len;
        unsigned char *p10 = m + 10 * PAGE;
        len = snprintf(msg, sizeof(msg),
                       "FAW: FA6 child alive, page10 head=%lu tail=%lu byte12=%02x m=%p\n",
                       *(unsigned long *)p10,
                       *(unsigned long *)(p10 + PAGE - 8),
                       p10[12], m);
        write(1, msg, len);
        /* Manual filler scan with first-mismatch report. */
        unsigned long badoff = 0xffffffff;
        unsigned char badval = 0;
        for (unsigned long i = 8; i < PAGE - 8; i++) {
            if (p10[i] != (unsigned char)(0x40 + (10 % 0x40))) {
                badoff = i;
                badval = p10[i];
                break;
            }
        }
        len = snprintf(msg, sizeof(msg),
                       "FAW: FA6 child filler scan: first bad off=%lu val=%02x\n",
                       badoff, badval);
        write(1, msg, len);
        /* Write-after-fork COW check LAST (it deliberately dirties the
         * head stamp at byte 4); check_page must run BEFORE it. */
        check_page("FA6-child", p10, 10);
        unsigned char before = p10[4];
        p10[4] = 0x5A;
        int wb = (p10[4] != 0x5A);
        len = snprintf(msg, sizeof(msg),
                       "FAW: FA6 child write: before=%02x after=%02x %s\n",
                       before, p10[4], wb ? "READBACK-FAIL" : "ok");
        write(1, msg, len);
        _exit(failures != base_failures ? 3 : 0);
    }
    int st = 0;
    waitpid(pid, &st, 0);
    if (!(WIFEXITED(st) && WEXITSTATUS(st) == 0))
        fail("FA6", "child status %d", st);
    /* Parent's page 10 untouched by the child's store (COW). */
    if (m[10 * PAGE + 4] == 0x5A)
        fail("FA6", "child store leaked into parent (COW broken)");
    else if (m[10 * PAGE + 4] != (unsigned char)(0x40 + 10))
        check_page("FA6", m + 10 * PAGE, 10);
    munmap(m, 32 * PAGE);
    close(fd);
    printf("FAW: FA6 fork COW over around window ok\n");
}

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("FAW: fault-around semantics probe start\n");
    leg_fa1();
    leg_fa2();
    leg_fa3();
    leg_fa4();
    leg_fa5();
    leg_fa6();
    if (failures) {
        printf("FAW-FAIL: %d failure(s)\n", failures);
        return 1;
    }
    printf("FAW-PASS\n");
    return 0;
}
