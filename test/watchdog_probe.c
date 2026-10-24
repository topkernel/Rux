/*
 * watchdog_probe.c — /dev/watchdog Linux-parity probe.
 *
 * Two verdict groups:
 *
 *  1. STAT SHAPE — the b151 forensics shape: newfstatat of a MISSING
 *     leaf must fail with ENOENT, never ENOTDIR (OH watchdog_service
 *     WaitAtStartup / linker-namespace stat loops). Probes the raw
 *     syscall on devfs and tmpfs missing leaves, plus the classic
 *     middle-component-is-file shapes that MUST stay ENOTDIR
 *     (regression guard).
 *
 *  2. WATCHDOG DEVICE — the exact init_watchdog.c sequence: open,
 *     WDIOC_SETTIMEOUT/GETTIMEOUT round-trip, GETBOOTSTATUS, KEEPALIVE,
 *     GETSUPPORT, write() ping.
 *
 * Prints one ok/FAIL line per step and:
 *   WDG_PROBE RESULT: PASS n/n
 *   WDG_PROBE RESULT: FAIL p/n
 */
#include <fcntl.h>
#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/ioctl.h>
#include <unistd.h>

/* linux/watchdog.h (uapi) */
struct watchdog_info {
    uint32_t options;
    uint32_t firmware_version;
    uint8_t identity[32];
};
#define WDIOC_GETSUPPORT      _IOR('W', 0, struct watchdog_info)
#define WDIOC_GETSTATUS       _IOR('W', 1, int)
#define WDIOC_GETBOOTSTATUS   _IOR('W', 2, int)
#define WDIOC_SETOPTIONS      _IOR('W', 4, int)
#define WDIOC_KEEPALIVE       _IO('W', 5)
#define WDIOC_SETTIMEOUT      _IOWR('W', 6, int)
#define WDIOC_GETTIMEOUT      _IOR('W', 7, int)
#define WDIOC_SETPRETIMEOUT   _IOWR('W', 8, int)
#define WDIOC_GETPRETIMEOUT   _IOR('W', 9, int)
#define WDIOC_GETTIMELEFT     _IOR('W', 10, int)

#define SELF_FILE "/test/watchdog_probe"

static int total, passed;

static void say(const char *s)
{
    write(1, s, strlen(s));
}

static void check(int ok, const char *what)
{
    total++;
    if (ok) {
        passed++;
        say("ok   ");
    } else {
        say("FAIL ");
    }
    say(what);
    say("\n");
}

static long raw_fstatat(const char *path, struct stat *st, int flags)
{
    return syscall(SYS_newfstatat, -100 /* AT_FDCWD */, path, st, flags);
}

static void expect_enoent(const char *path, const char *what)
{
    struct stat st;
    errno = 0;
    long r = raw_fstatat(path, &st, 0);
    check(r == -1 && errno == ENOENT, what);
}

int main(void)
{
    struct stat st;

    /* ---- group 1: stat shapes ------------------------------------ */
    expect_enoent("/dev/watchdog_no_such", "raw stat missing devfs leaf -> ENOENT");
    expect_enoent("/no_such_dir_at_all/leaf", "raw stat missing dir -> ENOENT");
    expect_enoent("/tmp/no_such_leaf", "raw stat missing tmpfs leaf -> ENOENT");

    errno = 0;
    if (stat("/dev/watchdog", &st) == 0) {
        check((st.st_mode & 0170000) == 020000, "stat /dev/watchdog is char dev");
    } else {
        check(0, "stat /dev/watchdog succeeds");
    }

    /* middle component is a regular file: MUST be ENOTDIR (LTP parity) */
    errno = 0;
    long r = raw_fstatat(SELF_FILE "/x", &st, 0);
    check(r == -1 && errno == ENOTDIR, "walk into regular file -> ENOTDIR (parity)");

    /* trailing slash on a regular file: MUST be ENOTDIR (LTP parity) */
    errno = 0;
    r = stat(SELF_FILE "/", &st);
    check(r == -1 && errno == ENOTDIR, "stat 'file/' -> ENOTDIR (parity)");

    /* ---- group 2: watchdog device ------------------------------- */
    int fd = open("/dev/watchdog", O_RDWR | O_CLOEXEC);
    check(fd >= 0, "open /dev/watchdog O_RDWR");

    if (fd >= 0) {
        int t = 23;
        check(ioctl(fd, WDIOC_SETTIMEOUT, &t) == 0, "WDIOC_SETTIMEOUT 23");
        t = -1;
        check(ioctl(fd, WDIOC_GETTIMEOUT, &t) == 0 && t == 23,
              "WDIOC_GETTIMEOUT == 23");

        check(ioctl(fd, WDIOC_KEEPALIVE) == 0, "WDIOC_KEEPALIVE");

        int bs = -1;
        check(ioctl(fd, WDIOC_GETBOOTSTATUS, &bs) == 0 && bs == 0,
              "WDIOC_GETBOOTSTATUS == 0");

        struct watchdog_info wi;
        memset(&wi, 0, sizeof(wi));
        check(ioctl(fd, WDIOC_GETSUPPORT, &wi) == 0 &&
              strlen((char *)wi.identity) > 0,
              "WDIOC_GETSUPPORT identity");

        int pre = 7;
        check(ioctl(fd, WDIOC_SETPRETIMEOUT, &pre) == 0, "WDIOC_SETPRETIMEOUT 7");
        pre = -1;
        check(ioctl(fd, WDIOC_GETPRETIMEOUT, &pre) == 0 && pre == 7,
              "WDIOC_GETPRETIMEOUT == 7");

        check(write(fd, "p", 1) == 1, "write() keepalive ping");

        close(fd);
    }

    say(passed == total ? "WDG_PROBE RESULT: PASS " : "WDG_PROBE RESULT: FAIL ");
    char nbuf[8];
    int n = 0;
    nbuf[n++] = '0' + (char)(passed / 10);
    nbuf[n++] = '0' + (char)(passed % 10);
    nbuf[n++] = '/';
    nbuf[n++] = '0' + (char)(total / 10);
    nbuf[n++] = '0' + (char)(total % 10);
    nbuf[n++] = '\n';
    write(1, nbuf, n);
    return passed == total ? 0 : 1;
}
