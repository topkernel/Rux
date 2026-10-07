/*
 * oh2_probe.c — OpenHarmony Phase 2 pre-work verification probe.
 *
 * Exercises the three Phase 2 items that landed in this worktree:
 *
 *   1. memfd_create (NR 279) wiring + fcntl F_ADD_SEALS/F_GET_SEALS
 *   2. /dev/ashmem (SET_NAME/SET_SIZE/PIN/UNPIN/GET_PIN_STATUS + mmap)
 *   3. /dev/access_token_id (GET/SET TOKENID/FTOKENID, fork semantics)
 *
 * Prints one line per check ("ok ..." / "FAIL ...") and a final verdict:
 *
 *   OH2_PROBE RESULT: PASS <n>/<n>
 *   OH2_PROBE RESULT: FAIL <passed>/<total>
 *
 * Build (musl static, see toolchain/README.md):
 *   riscv64-linux-gnu-gcc -static -nostdlib -I toolchain/.../include \
 *       -o oh2_probe oh2_probe.c toolchain/.../lib/crt1.o .../lib/libc.a -lgcc
 *
 * Runs as init (init=/test/oh2_probe) so it is uid 0 with full caps.
 */

#include <fcntl.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <sys/ioctl.h>

#ifndef MFD_CLOEXEC
#define MFD_CLOEXEC       0x0001U
#endif
#ifndef MFD_ALLOW_SEALING
#define MFD_ALLOW_SEALING 0x0002U
#endif

#ifndef F_ADD_SEALS
#define F_ADD_SEALS 1033
#endif
#ifndef F_GET_SEALS
#define F_GET_SEALS 1034
#endif
#ifndef F_SEAL_SEAL
#define F_SEAL_SEAL   0x0001
#define F_SEAL_SHRINK 0x0002
#define F_SEAL_GROW   0x0004
#define F_SEAL_WRITE  0x0008
#endif

/* ashmem UAPI (linux/drivers/staging/android/uapi/ashmem.h) */
#define ASHMEM_NAME_LEN 256
struct ashmem_pin {
    uint32_t offset;
    uint32_t len;
};
#define ASHMEM_SET_NAME        _IOW(0x77, 1, char[ASHMEM_NAME_LEN])
#define ASHMEM_GET_NAME        _IOR(0x77, 2, char[ASHMEM_NAME_LEN])
#define ASHMEM_SET_SIZE        _IOW(0x77, 3, size_t)
#define ASHMEM_GET_SIZE        _IO(0x77, 4)
#define ASHMEM_SET_PROT_MASK   _IOW(0x77, 5, unsigned long)
#define ASHMEM_GET_PROT_MASK   _IO(0x77, 6)
#define ASHMEM_PIN             _IOW(0x77, 7, struct ashmem_pin)
#define ASHMEM_UNPIN           _IOW(0x77, 8, struct ashmem_pin)
#define ASHMEM_GET_PIN_STATUS  _IO(0x77, 9)
#define ASHMEM_PURGE_ALL_CACHES _IO(0x77, 10)
#define ASHMEM_IS_UNPINNED 0
#define ASHMEM_IS_PINNED   1
#define ASHMEM_NOT_PURGED  0

/* accesstokenid UAPI (OH drivers/accesstokenid) */
#define ACCESS_TOKENID_GET_TOKENID  _IOR('A', 1, uint64_t)
#define ACCESS_TOKENID_SET_TOKENID  _IOW('A', 2, uint64_t)
#define ACCESS_TOKENID_GET_FTOKENID _IOR('A', 3, uint64_t)
#define ACCESS_TOKENID_SET_FTOKENID _IOW('A', 4, uint64_t)

static int total, passed;

static void ok(const char *what)
{
    passed++;
    total++;
    printf("ok   %s\n", what);
}

static void bad(const char *what, long got, long want)
{
    total++;
    printf("FAIL %s (got %ld / errno %d, want %ld)\n", what, got, errno,
           want);
}

/* Check an expression returns the expected value (>=0 or -1+errno match). */
#define CHECK_EQ(expr, want, name) \
    do { long _g = (long)(expr); \
         if (_g == (long)(want)) ok(name); \
         else bad(name, _g, (long)(want)); } while (0)

#define CHECK_ERR(expr, wanterrno, name) \
    do { errno = 0; long _g = (long)(expr); \
         if (_g == -1 && errno == (wanterrno)) ok(name); \
         else bad(name, _g, -1000 - (wanterrno)); } while (0)

static long sys_memfd(const char *name, unsigned int flags)
{
    return syscall(SYS_memfd_create, name, flags);
}

/* ================= memfd ================= */
static void memfd_tests(void)
{
    char buf[64];
    struct stat st;

    int fd = (int)sys_memfd(NULL, 0);
    if (fd < 0) { bad("memfd_create(NULL,0)", fd, 1); goto out; }
    ok("memfd_create(NULL,0) returns fd");

    CHECK_ERR(sys_memfd("x", 0xffffffffU), EINVAL, "memfd_create bad flags EINVAL");

    CHECK_EQ(write(fd, "hello memfd", 11), 11, "memfd write");
    CHECK_EQ(lseek(fd, 0, SEEK_SET), 0, "memfd lseek");
    memset(buf, 0, sizeof(buf));
    CHECK_EQ(read(fd, buf, 11), 11, "memfd read");
    if (memcmp(buf, "hello memfd", 11) == 0) ok("memfd read-back content");
    else bad("memfd read-back content", buf[0], 'h');

    CHECK_EQ(ftruncate(fd, 4096), 0, "memfd ftruncate grow");
    CHECK_EQ(fstat(fd, &st), 0, "memfd fstat");
    CHECK_EQ((long)st.st_size, 4096, "memfd st_size == 4096");
    CHECK_EQ((long)(st.st_mode & S_IFMT), S_IFREG, "memfd S_IFREG");

    /* Seals: a memfd WITHOUT MFD_ALLOW_SEALING rejects F_ADD_SEALS */
    CHECK_ERR(fcntl(fd, F_ADD_SEALS, F_SEAL_GROW), EPERM,
              "F_ADD_SEALS without MFD_ALLOW_SEALING EPERM");
    CHECK_EQ(fcntl(fd, F_GET_SEALS), 0, "F_GET_SEALS == 0 (no seals)");
    CHECK_ERR(fcntl(fd, F_ADD_SEALS, 0x20), EINVAL,
              "F_ADD_SEALS unknown bit EINVAL");

    close(fd);

    /* Sealing memfd: GROW seal blocks growth, SHRINK still allowed */
    int sfd = (int)sys_memfd("sealed", MFD_ALLOW_SEALING);
    if (sfd < 0) { bad("memfd_create(allow_sealing)", sfd, 1); return; }
    ok("memfd_create(MFD_ALLOW_SEALING)");
    CHECK_EQ(ftruncate(sfd, 4096), 0, "ftruncate before GROW seal");

    CHECK_EQ(fcntl(sfd, F_ADD_SEALS, F_SEAL_GROW), 0, "F_ADD_SEALS GROW");
    CHECK_EQ(fcntl(sfd, F_GET_SEALS), F_SEAL_GROW, "F_GET_SEALS == GROW");
    CHECK_ERR(ftruncate(sfd, 8192), EPERM, "sealed ftruncate grow EPERM");
    CHECK_EQ(ftruncate(sfd, 512), 0, "sealed ftruncate shrink ok");

    CHECK_EQ(fcntl(sfd, F_ADD_SEALS, F_SEAL_SEAL), 0, "F_ADD_SEALS SEAL");
    CHECK_ERR(fcntl(sfd, F_ADD_SEALS, F_SEAL_SHRINK), EPERM,
              "SEAL-sealed F_ADD_SEALS EPERM");
    close(sfd);

    /* F_SEAL_WRITE blocks write(2) */
    int wfd = (int)sys_memfd("wsealed", MFD_ALLOW_SEALING);
    CHECK_EQ((long)wfd >= 0, 1, "memfd_create for WRITE seal");
    CHECK_EQ(write(wfd, "data", 4), 4, "write before WRITE seal");
    CHECK_EQ(fcntl(wfd, F_ADD_SEALS, F_SEAL_WRITE), 0, "F_ADD_SEALS WRITE");
    CHECK_ERR(write(wfd, "more", 4), EPERM, "write after WRITE seal EPERM");
    CHECK_ERR(ftruncate(wfd, 8), EPERM, "ftruncate after WRITE seal EPERM");
    close(wfd);

    /* Adding F_SEAL_WRITE while a writable MAP_SHARED mapping exists */
    int mfd = (int)sys_memfd("mapped", MFD_ALLOW_SEALING);
    CHECK_EQ((long)mfd >= 0, 1, "memfd_create for mapping test");
    CHECK_EQ(ftruncate(mfd, 4096), 0, "ftruncate before mmap");
    void *p = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, mfd, 0);
    if (p != MAP_FAILED) {
        ok("memfd mmap MAP_SHARED RW");
        CHECK_ERR(fcntl(mfd, F_ADD_SEALS, F_SEAL_WRITE), EBUSY,
                  "F_ADD_SEALS WRITE with live mapping EBUSY");
        munmap(p, 4096);
    } else {
        bad("memfd mmap MAP_SHARED RW", -1, 0);
    }
    /* After munmap the seal goes through */
    CHECK_EQ(fcntl(mfd, F_ADD_SEALS, F_SEAL_WRITE), 0,
             "F_ADD_SEALS WRITE after munmap");
    close(mfd);

    /* F_GET_SEALS on a non-memfd fd is EINVAL */
    int nfd = open("/dev/null", O_RDONLY);
    if (nfd >= 0) {
        CHECK_ERR(fcntl(nfd, F_GET_SEALS), EINVAL,
                  "F_GET_SEALS on /dev/null EINVAL");
        close(nfd);
    }
    return;
out:
    return;
}

/* ================= ashmem ================= */
static void ashmem_tests(void)
{
    int fd = open("/dev/ashmem", O_RDWR);
    if (fd < 0) { bad("open /dev/ashmem", fd, 1); return; }
    ok("open /dev/ashmem");

    CHECK_EQ(ioctl(fd, ASHMEM_GET_SIZE), 0, "GET_SIZE initial 0");
    CHECK_EQ(ioctl(fd, ASHMEM_SET_SIZE, 8192), 0, "SET_SIZE 8192");
    CHECK_EQ(ioctl(fd, ASHMEM_GET_SIZE), 8192, "GET_SIZE 8192");

    CHECK_EQ(ioctl(fd, ASHMEM_SET_NAME, "oh2probe"), 0, "SET_NAME");
    char name[ASHMEM_NAME_LEN];
    memset(name, 0, sizeof(name));
    CHECK_EQ(ioctl(fd, ASHMEM_GET_NAME, name), 0, "GET_NAME ret");
    if (strcmp(name, "oh2probe") == 0) ok("GET_NAME content");
    else bad("GET_NAME content", name[0], 'o');

    /* mmap + write/read loop */
    uint8_t *m = mmap(NULL, 8192, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (m == MAP_FAILED) { bad("ashmem mmap", -1, 0); close(fd); return; }
    ok("ashmem mmap MAP_SHARED RW");
    for (int i = 0; i < 8192; i++) m[i] = (uint8_t)(i ^ 0xa5);
    int mism = 0;
    for (int i = 0; i < 8192; i++) if (m[i] != (uint8_t)(i ^ 0xa5)) mism++;
    CHECK_EQ(mism, 0, "ashmem mmap write/read loop 8192B");

    /* read(2) path sees the mapped content */
    uint8_t rb[64];
    CHECK_EQ(lseek(fd, 0, SEEK_SET), 0, "ashmem lseek SEEK_SET");
    CHECK_EQ(read(fd, rb, 64), 64, "ashmem read 64");
    mism = 0;
    for (int i = 0; i < 64; i++) if (rb[i] != (uint8_t)(i ^ 0xa5)) mism++;
    CHECK_EQ(mism, 0, "ashmem read content == mapped content");

    /* fork sharing: child writes into page 1, parent must see it */
    memset(m + 4096, 0, 4096);
    pid_t pid = fork();
    if (pid == 0) {
        m[4096] = 0xCA; m[4097] = 0xFE; m[8191] = 0x42;
        _exit(0);
    }
    int wst;
    waitpid(pid, &wst, 0);
    CHECK_EQ(m[4096] == 0xCA && m[4097] == 0xFE && m[8191] == 0x42, 1,
             "ashmem MAP_SHARED visible across fork");

    /* pin/unpin accounting */
    struct ashmem_pin whole = { 0, 0 };
    struct ashmem_pin pg0 = { 0, 4096 };
    struct ashmem_pin pg1 = { 4096, 4096 };
    struct ashmem_pin bad_pin = { 1, 4096 };

    CHECK_EQ(ioctl(fd, ASHMEM_GET_PIN_STATUS, &whole), ASHMEM_IS_PINNED,
             "pin_status whole initially PINNED");
    CHECK_EQ(ioctl(fd, ASHMEM_UNPIN, &pg0), 0, "unpin page 0");
    CHECK_EQ(ioctl(fd, ASHMEM_GET_PIN_STATUS, &pg0), ASHMEM_IS_UNPINNED,
             "pin_status page0 UNPINNED");
    CHECK_EQ(ioctl(fd, ASHMEM_GET_PIN_STATUS, &pg1), ASHMEM_IS_PINNED,
             "pin_status page1 still PINNED");
    CHECK_EQ(ioctl(fd, ASHMEM_PIN, &pg0), ASHMEM_NOT_PURGED,
             "pin page 0 (NOT_PURGED)");
    CHECK_EQ(ioctl(fd, ASHMEM_GET_PIN_STATUS, &pg0), ASHMEM_IS_PINNED,
             "pin_status page0 PINNED again");
    CHECK_EQ(ioctl(fd, ASHMEM_UNPIN, &whole), 0, "unpin whole (len=0)");
    CHECK_EQ(ioctl(fd, ASHMEM_GET_PIN_STATUS, &whole), ASHMEM_IS_UNPINNED,
             "pin_status whole UNPINNED");
    CHECK_EQ(ioctl(fd, ASHMEM_PIN, &pg0), ASHMEM_NOT_PURGED,
             "pin inside unpinned whole (hole punch)");
    CHECK_EQ(ioctl(fd, ASHMEM_GET_PIN_STATUS, &pg1), ASHMEM_IS_UNPINNED,
             "page1 still unpinned after partial pin");
    CHECK_ERR(ioctl(fd, ASHMEM_PIN, &bad_pin), EINVAL, "unaligned pin EINVAL");
    CHECK_EQ(ioctl(fd, ASHMEM_PURGE_ALL_CACHES), 0,
             "PURGE_ALL_CACHES as root");

    /* frozen state after mmap */
    CHECK_ERR(ioctl(fd, ASHMEM_SET_SIZE, 16384), EINVAL, "SET_SIZE after mmap EINVAL");
    CHECK_ERR(ioctl(fd, ASHMEM_SET_NAME, "nope"), EINVAL, "SET_NAME after mmap EINVAL");

    munmap(m, 8192);
    close(fd);

    /* prot mask: narrow to read-only, then a writable mmap must fail */
    int pfd = open("/dev/ashmem", O_RDWR);
    CHECK_EQ((long)pfd >= 0, 1, "reopen /dev/ashmem (prot test)");
    CHECK_EQ(ioctl(pfd, ASHMEM_SET_SIZE, 4096), 0, "SET_SIZE 4096 (prot test)");
    CHECK_EQ(ioctl(pfd, ASHMEM_SET_PROT_MASK, PROT_READ), 0, "SET_PROT_MASK READ");
    CHECK_EQ(ioctl(pfd, ASHMEM_GET_PROT_MASK), PROT_READ, "GET_PROT_MASK READ");
    CHECK_ERR(ioctl(pfd, ASHMEM_SET_PROT_MASK, PROT_READ | PROT_WRITE), EINVAL,
              "SET_PROT_MASK widen EINVAL");
    void *rp = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, pfd, 0);
    if (rp == MAP_FAILED && errno == EPERM)
        ok("writable mmap with READ-only mask EPERM");
    else
        bad("writable mmap with READ-only mask EPERM", rp != MAP_FAILED, -1000 - EPERM);
    void *ro = mmap(NULL, 4096, PROT_READ, MAP_SHARED, pfd, 0);
    CHECK_EQ(ro != MAP_FAILED, 1, "read-only mmap allowed");
    if (ro != MAP_FAILED) munmap(ro, 4096);
    close(pfd);

    /* pin before mmap: fresh fd, PIN without a mapping is EINVAL */
    int nfd2 = open("/dev/ashmem", O_RDWR);
    CHECK_EQ(ioctl(nfd2, ASHMEM_SET_SIZE, 4096), 0, "SET_SIZE (pin-pre-mmap)");
    CHECK_ERR(ioctl(nfd2, ASHMEM_PIN, &pg0), EINVAL, "PIN before mmap EINVAL");
    close(nfd2);

    /* mmap bigger than the area */
    int bfd = open("/dev/ashmem", O_RDWR);
    CHECK_EQ(ioctl(bfd, ASHMEM_SET_SIZE, 4096), 0, "SET_SIZE (oversize test)");
    void *over = mmap(NULL, 8192, PROT_READ | PROT_WRITE, MAP_SHARED, bfd, 0);
    if (over == MAP_FAILED && errno == EINVAL)
        ok("mmap larger than area EINVAL");
    else
        bad("mmap larger than area EINVAL", over != MAP_FAILED, -1000 - EINVAL);
    close(bfd);

    /* mmap before SET_SIZE */
    int zfd = open("/dev/ashmem", O_RDWR);
    void *zm = mmap(NULL, 4096, PROT_READ, MAP_SHARED, zfd, 0);
    if (zm == MAP_FAILED && errno == EINVAL)
        ok("mmap without SET_SIZE EINVAL");
    else
        bad("mmap without SET_SIZE EINVAL", zm != MAP_FAILED, -1000 - EINVAL);
    close(zfd);

    /* MAP_PRIVATE: writes stay private (COW), parent unaffected */
    int prfd = open("/dev/ashmem", O_RDWR);
    CHECK_EQ(ioctl(prfd, ASHMEM_SET_SIZE, 4096), 0, "SET_SIZE (private test)");
    uint8_t *pm = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE, prfd, 0);
    CHECK_EQ(pm != MAP_FAILED, 1, "ashmem mmap MAP_PRIVATE");
    if (pm != MAP_FAILED) {
        pm[0] = 1;
        pid = fork();
        if (pid == 0) {
            pm[0] = 9;
            _exit(0);
        }
        waitpid(pid, &wst, 0);
        CHECK_EQ(pm[0], 1, "MAP_PRIVATE child write stays private");
        munmap(pm, 4096);
    }
    close(prfd);
}

/* ================= access_token_id ================= */
static void tokenid_tests(void)
{
    int fd = open("/dev/access_token_id", O_RDWR);
    if (fd < 0) { bad("open /dev/access_token_id", fd, 1); return; }
    ok("open /dev/access_token_id");

    uint64_t tok = 0, ftok = 0;
    CHECK_EQ(ioctl(fd, ACCESS_TOKENID_GET_TOKENID, &tok), 0, "GET_TOKENID ret");
    CHECK_EQ((long)tok, 0, "initial token 0");

    uint64_t want_tok = 0x1234abcdULL;
    CHECK_EQ(ioctl(fd, ACCESS_TOKENID_SET_TOKENID, &want_tok), 0,
             "SET_TOKENID");
    tok = 0;
    CHECK_EQ(ioctl(fd, ACCESS_TOKENID_GET_TOKENID, &tok), 0, "GET_TOKENID ret 2");
    CHECK_EQ((long)tok, 0x1234abcd, "token round-trip");

    /* fork: token inherited, ftoken cleared in the child */
    uint64_t want_ftok = 0x77ULL;
    CHECK_EQ(ioctl(fd, ACCESS_TOKENID_SET_FTOKENID, &want_ftok), 0, "SET_FTOKENID");
    ftok = 0;
    CHECK_EQ(ioctl(fd, ACCESS_TOKENID_GET_FTOKENID, &ftok), 0, "GET_FTOKENID ret");
    CHECK_EQ((long)ftok, 0x77, "ftoken round-trip");

    pid_t pid = fork();
    if (pid == 0) {
        uint64_t ct = 1, cf = 1;
        ioctl(fd, ACCESS_TOKENID_GET_TOKENID, &ct);
        ioctl(fd, ACCESS_TOKENID_GET_FTOKENID, &cf);
        /* child: token inherited, ftoken cleared */
        _exit((ct == 0x1234abcd && cf == 0) ? 0 : 1);
    }
    int wst;
    waitpid(pid, &wst, 0);
    CHECK_EQ(WEXITSTATUS(wst), 0, "fork inherits token, clears ftoken");

    /* parent state untouched by the child's reads */
    tok = 0; ftok = 0;
    ioctl(fd, ACCESS_TOKENID_GET_TOKENID, &tok);
    ioctl(fd, ACCESS_TOKENID_GET_FTOKENID, &ftok);
    CHECK_EQ((long)tok == 0x1234abcd && (long)ftok == 0x77, 1,
             "parent tokens unchanged by fork");

    /* unknown ioctl number (with a valid payload pointer) -> ENOTTY;
     * NULL payload is EINVAL first (upstream checks the pointer first) */
    CHECK_ERR(ioctl(fd, _IO('A', 99), &tok), ENOTTY, "unknown tokenid ioctl ENOTTY");
    CHECK_ERR(ioctl(fd, ACCESS_TOKENID_GET_TOKENID, NULL), EINVAL,
              "NULL payload EINVAL");

    close(fd);
}

int main(void)
{
    printf("== OH2 probe: memfd ==\n");
    memfd_tests();
    printf("== OH2 probe: ashmem ==\n");
    ashmem_tests();
    printf("== OH2 probe: access_token_id ==\n");
    tokenid_tests();

    printf("OH2_PROBE RESULT: %s %d/%d\n",
           passed == total ? "PASS" : "FAIL", passed, total);
    return passed == total ? 0 : 1;
}
