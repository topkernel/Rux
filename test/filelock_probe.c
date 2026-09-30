/*
 * filelock_probe.c — static verification probe for flock(2) + fcntl
 * record locks (P0-5), mirroring test/inotify_probe.c's discipline.
 *
 * Build: riscv64-linux-gnu-gcc -static -O2 -o probe filelock_probe.c
 * Reference (Linux semantics via qemu-user against the host kernel):
 *   LOCKFILE=./lockdata qemu-riscv64 ./probe   # expect 24/24 PASS
 * Rux (boots as init; the kernel opens the console for stdout):
 *   image with /probe at the root, then
 *   qemu ... -append "root=/dev/vda rw init=/probe console=ttyS0"
 *   # expect 24/24 PASS and PROBE-SUMMARY pass=24 fail=0
 *
 * Matrix (each case prints PASS/FAIL; any FAIL fails the run):
 *  flock: basic EX/UN/relock; two-opens conflict (EWOULDBLOCK);
 *  SH coexist + upgrade blocked while another SH held, then granted;
 *  upgrade/downgrade via the same description; dup'd fd shares the
 *  description (re-lock, convert, UN releases for both); fork shares
 *  the description (child conversion visible to parent); release on
 *  close; blocking acquisition unblocks on holder exit (>=0.5s);
 *  EBADF/EINVAL; flock independent of POSIX locks (both directions).
 *  fcntl: own locks never self-conflict (GETLK -> F_UNLCK, re-set
 *  overlaps); cross-process W/R conflicts with EAGAIN and exact
 *  boundary touching ([0,10) vs [10,20)); F_GETLK backfill
 *  (l_type/l_start/l_len/l_pid/l_whence, F_UNLCK outside); read locks
 *  shared across processes; unlock splits [0,100) around [40,60) with
 *  exact fragments; adjacent same-type locks coalesce; same-process
 *  replacement/downgrade of a sub-range; close(ANY fd) drops all the
 *  closer's record locks on the file while its flock survives;
 *  l_len=0 locks to EOF (blocks [1000,1010)); SEEK_END-relative
 *  request resolves against file size; F_SETLKW blocks then granted on
 *  release; F_SETLKW interrupted by a no-SA_RESTART signal -> EINTR;
 *  access-mode checks (W on O_RDONLY / R on O_WRONLY -> EBADF);
 *  locks not inherited across fork (child GETLK reports parent pid,
 *  child SETLK conflicts, child UNLK does not free the parent's
 *  lock); owner exit releases everything.
 */
#define _GNU_SOURCE
#include <stdarg.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#include <errno.h>

static int g_pass = 0, g_fail = 0;

static void ok(const char *name) { printf("PASS %s\n", name); g_pass++; }

static void bad(const char *name, const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    printf("FAIL %s: ", name);
    vprintf(fmt, ap);
    printf("\n");
    va_end(ap);
    g_fail++;
}

static double now_s(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec / 1e9;
}

#define LOCKFILE_PATH "/lockdata"
static const char *LOCKFILE = LOCKFILE_PATH;

/* ---- child-orchestration helpers ---- */

/* Fork a child running fn() (which takes a lock and returns 0). The child
 * signals readiness through a pipe, then holds the lock until a byte
 * arrives on a second pipe. Returns (pid, go_fd). */
static pid_t locked_child2(int (*fn)(void), int *go_fd)
{
    int ready[2], go[2];
    if (pipe(ready) != 0 || pipe(go) != 0) { perror("pipe"); _exit(98); }
    pid_t pid = fork();
    if (pid == 0) {
        close(ready[0]); close(go[1]);
        int r = fn();
        char c = r ? 'F' : 'L';
        if (write(ready[1], &c, 1) < 1) _exit(96);
        if (r == 0) { char g; if (read(go[0], &g, 1) < 0) _exit(96); }
        _exit(0);
    }
    close(ready[1]); close(go[0]);
    char c;
    if (read(ready[0], &c, 1) != 1 || c != 'L') {
        printf("FATAL locked_child2: child did not acquire lock\n");
        _exit(99);
    }
    close(ready[0]);
    *go_fd = go[1];
    return pid;
}

static void release_child(int go_fd, pid_t pid)
{
    char c = 'g';
    if (write(go_fd, &c, 1) < 0) { /* closing still wakes EOF */ }
    close(go_fd);
    waitpid(pid, NULL, 0);
}

/* ---- lock-taking child workers ---- */

__attribute__((unused)) static int cf_noop(void) { return 0; }

__attribute__((unused)) static int cf_flock_ex(void) /* flock EX on fresh open, hold */
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0 || flock(fd, LOCK_EX) != 0) return 1;
    return 0;
}

static int cf_posix_wrlck_all(void)   /* F_WRLCK whole file, hold */
{
    int fd = open(LOCKFILE, O_RDWR);
    struct flock fl = { F_WRLCK, SEEK_SET, 0, 0, 0 };
    if (fd < 0 || fcntl(fd, F_SETLK, &fl) != 0) return 1;
    return 0;
}

static int cf_flock_ex_1500(void)     /* flock EX, self-releases after 1.5s */
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0 || flock(fd, LOCK_EX) != 0) return 1;
    usleep(1500 * 1000);
    return 0;
}

static int cf_wrlck_10_20(void)       /* F_WRLCK [10,20), hold */
{
    int fd = open(LOCKFILE, O_RDWR);
    struct flock fl = { F_WRLCK, SEEK_SET, 10, 10, 0 };
    if (fd < 0 || fcntl(fd, F_SETLK, &fl) != 0) return 1;
    return 0;
}

static int cf_wrlck_10_20_1500(void)  /* F_WRLCK [10,20), release after 1.5s */
{
    int fd = open(LOCKFILE, O_RDWR);
    struct flock fl = { F_WRLCK, SEEK_SET, 10, 10, 0 };
    if (fd < 0 || fcntl(fd, F_SETLK, &fl) != 0) return 1;
    usleep(1500 * 1000);
    return 0;
}

static int cf_wrlck_0_10_3000(void)   /* F_WRLCK [0,10), release after 3s */
{
    int fd = open(LOCKFILE, O_RDWR);
    struct flock fl = { F_WRLCK, SEEK_SET, 0, 10, 0 };
    if (fd < 0 || fcntl(fd, F_SETLK, &fl) != 0) return 1;
    usleep(3000 * 1000);
    return 0;
}

static int cf_rdlck_0_100(void)       /* F_RDLCK [0,100), hold */
{
    int fd = open(LOCKFILE, O_RDWR);
    struct flock fl = { F_RDLCK, SEEK_SET, 0, 100, 0 };
    if (fd < 0 || fcntl(fd, F_SETLK, &fl) != 0) return 1;
    return 0;
}

static volatile sig_atomic_t g_got_sig;
static void sig_handler(int s) { (void)s; g_got_sig = 1; }

static void set_lock(int fd, short type, off_t start, off_t len)
{
    struct flock fl = { type, SEEK_SET, start, len, 0 };
    if (fcntl(fd, F_SETLK, &fl) != 0)
        printf("WARN set_lock(%d,%d,%ld,%ld) errno=%d\n", fd, type, (long)start, (long)len, errno);
}

/* first conflicting lock for a W request over [start,start+len), or F_UNLCK */
static struct flock get_first_conflict(int fd, off_t start, off_t len)
{
    struct flock fl = { F_WRLCK, SEEK_SET, start, len, 0 };
    if (fcntl(fd, F_GETLK, &fl) != 0)
        fl.l_type = (short)-1;
    return fl;
}

/* F_GETLK from a FRESH process: a process's own locks are invisible to
 * its own F_GETLK, so the lock holder must never probe its ranges. */
static int child_getlk(off_t start, off_t len, struct flock *out)
{
    int p[2];
    if (pipe(p) != 0) return -1;
    pid_t pid = fork();
    if (pid == 0) {
        close(p[0]);
        int fd = open(LOCKFILE, O_RDWR);
        struct flock fl = { F_WRLCK, SEEK_SET, start, len, 0 };
        if (fd < 0 || fcntl(fd, F_GETLK, &fl) != 0) _exit(1);
        if (write(p[1], &fl, sizeof fl) != (int)sizeof fl) _exit(2);
        _exit(0);
    }
    close(p[1]);
    int n = read(p[0], out, sizeof *out);
    close(p[0]);
    int st;
    waitpid(pid, &st, 0);
    if (n != (int)sizeof *out || !(WIFEXITED(st) && WEXITSTATUS(st) == 0))
        return -1;
    return 0;
}

/* ========================================================================= */
/* flock tests                                                               */
/* ========================================================================= */

static void t_flock_basic(void)
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0) { bad("flock-basic", "open errno=%d", errno); return; }
    if (flock(fd, LOCK_EX) != 0) { bad("flock-basic", "EX errno=%d", errno); goto out; }
    if (flock(fd, LOCK_UN) != 0) { bad("flock-basic", "UN errno=%d", errno); goto out; }
    if (flock(fd, LOCK_EX) != 0) { bad("flock-basic", "re-EX errno=%d", errno); goto out; }
    if (flock(fd, LOCK_UN) != 0) { bad("flock-basic", "re-UN errno=%d", errno); goto out; }
    ok("flock-basic");
out:
    flock(fd, LOCK_UN);
    close(fd);
}

static void t_flock_conflict_two_opens(void)
{
    int a = open(LOCKFILE, O_RDWR), b = open(LOCKFILE, O_RDWR);
    if (a < 0 || b < 0) { bad("flock-conflict-two-opens", "open errno=%d", errno); return; }
    if (flock(a, LOCK_EX) != 0) { bad("flock-conflict-two-opens", "a EX errno=%d", errno); goto out; }
    errno = 0;
    if (flock(b, LOCK_EX | LOCK_NB) == 0) { bad("flock-conflict-two-opens", "second description EX|NB granted"); goto out; }
    if (errno != EWOULDBLOCK) { bad("flock-conflict-two-opens", "errno=%d want EWOULDBLOCK(%d)", errno, EWOULDBLOCK); goto out; }
    ok("flock-conflict-two-opens");
out:
    flock(a, LOCK_UN); flock(b, LOCK_UN);
    close(a); close(b);
}

static void t_flock_sh_shared_and_upgrade(void)
{
    int a = open(LOCKFILE, O_RDWR), b = open(LOCKFILE, O_RDWR);
    if (a < 0 || b < 0) { bad("flock-sh-upgrade", "open errno=%d", errno); return; }
    if (flock(a, LOCK_SH) != 0) { bad("flock-sh-upgrade", "a SH errno=%d", errno); goto out; }
    if (flock(b, LOCK_SH) != 0) { bad("flock-sh-upgrade", "b SH errno=%d (shared must coexist)", errno); goto out; }
    errno = 0;
    if (flock(a, LOCK_EX | LOCK_NB) == 0) { bad("flock-sh-upgrade", "upgrade granted while other SH held"); goto out; }
    if (errno != EWOULDBLOCK) { bad("flock-sh-upgrade", "upgrade errno=%d", errno); goto out; }
    flock(b, LOCK_UN);
    if (flock(a, LOCK_EX | LOCK_NB) != 0) { bad("flock-sh-upgrade", "upgrade after release errno=%d", errno); goto out; }
    if (flock(a, LOCK_SH) != 0) { bad("flock-sh-upgrade", "downgrade errno=%d", errno); goto out; }
    ok("flock-sh-upgrade");
out:
    flock(a, LOCK_UN); flock(b, LOCK_UN);
    close(a); close(b);
}

static void t_flock_dup_shares(void)
{
    int a = open(LOCKFILE, O_RDWR), b = open(LOCKFILE, O_RDWR);
    if (a < 0 || b < 0) { bad("flock-dup-shares", "open errno=%d", errno); return; }
    int d = dup(a);
    if (flock(a, LOCK_EX) != 0) { bad("flock-dup-shares", "a EX errno=%d", errno); goto out; }
    /* dup'd fd = same open file description: no conflict, may convert */
    if (flock(d, LOCK_EX | LOCK_NB) != 0) { bad("flock-dup-shares", "dup EX|NB errno=%d", errno); goto out; }
    if (flock(d, LOCK_SH) != 0) { bad("flock-dup-shares", "dup downgrade errno=%d", errno); goto out; }
    /* other description vs our (now shared) lock: EX still conflicts */
    errno = 0;
    if (flock(b, LOCK_EX | LOCK_NB) == 0 || errno != EWOULDBLOCK) {
        bad("flock-dup-shares", "b EX|NB vs SH errno=%d", errno); goto out;
    }
    /* UN via the dup releases the description's lock entirely */
    if (flock(d, LOCK_UN) != 0) { bad("flock-dup-shares", "dup UN errno=%d", errno); goto out; }
    if (flock(b, LOCK_EX | LOCK_NB) != 0) { bad("flock-dup-shares", "b EX after UN errno=%d", errno); goto out; }
    ok("flock-dup-shares");
out:
    flock(a, LOCK_UN); flock(b, LOCK_UN); flock(d, LOCK_UN);
    close(a); close(b); close(d);
}

static void t_flock_fork(void)
{
    int a = open(LOCKFILE, O_RDWR), b = open(LOCKFILE, O_RDWR);
    if (a < 0 || b < 0) { bad("flock-fork", "open errno=%d", errno); return; }
    if (flock(a, LOCK_EX) != 0) { bad("flock-fork", "a EX errno=%d", errno); goto out; }

    int go[2];
    if (pipe(go) != 0) _exit(96);
    pid_t pid = fork();
    if (pid == 0) {
        close(go[1]);
        /* inherited fd = same description: SH conversion must not conflict */
        if (flock(a, LOCK_SH) != 0) _exit(1);
        /* fresh open (different description) vs the shared-description SH:
         * EX conflicts */
        if (flock(b, LOCK_EX | LOCK_NB) == 0) _exit(2);
        char g;
        if (read(go[0], &g, 1) < 0) _exit(96);
        _exit(0);
    }
    close(go[1]);
    int st;
    waitpid(pid, &st, 0);
    if (!(WIFEXITED(st) && WEXITSTATUS(st) == 0)) {
        bad("flock-fork", "child st=0x%x", st);
        goto out;
    }
    /* child converted the shared description to SH; parent still holds it */
    errno = 0;
    if (flock(b, LOCK_EX | LOCK_NB) == 0 || errno != EWOULDBLOCK) {
        bad("flock-fork", "post-child EX|NB errno=%d", errno);
        goto out;
    }
    flock(a, LOCK_UN);
    if (flock(b, LOCK_EX | LOCK_NB) != 0) { bad("flock-fork", "final EX errno=%d", errno); goto out; }
    flock(b, LOCK_UN);
    ok("flock-fork");
out:
    flock(a, LOCK_UN); flock(b, LOCK_UN);
    close(a); close(b);
}

static void t_flock_release_on_close(void)
{
    int a = open(LOCKFILE, O_RDWR);
    if (a < 0) { bad("flock-release-on-close", "open errno=%d", errno); return; }
    if (flock(a, LOCK_EX) != 0) { bad("flock-release-on-close", "EX errno=%d", errno); close(a); return; }
    close(a);
    int b = open(LOCKFILE, O_RDWR);
    if (flock(b, LOCK_EX | LOCK_NB) != 0) { bad("flock-release-on-close", "lock survived close errno=%d", errno); close(b); return; }
    flock(b, LOCK_UN);
    close(b);
    ok("flock-release-on-close");
}

static void t_flock_blocking(void)
{
    /* child holds EX for ~1.5s via its own open then exits (close releases) */
    pid_t pid = fork();
    if (pid == 0) _exit(cf_flock_ex_1500() ? 1 : 0);
    usleep(300 * 1000);                     /* let the child take it */

    int fd = open(LOCKFILE, O_RDWR);
    double t0 = now_s();
    int r = flock(fd, LOCK_EX);             /* blocks until child exits */
    double dt = now_s() - t0;
    waitpid(pid, NULL, 0);
    if (r != 0) { bad("flock-blocking", "errno=%d", errno); close(fd); return; }
    if (dt < 0.5) { bad("flock-blocking", "returned too early dt=%.2fs", dt); close(fd); return; }
    if (dt > 30.0) { bad("flock-blocking", "took too long dt=%.2fs", dt); close(fd); return; }
    ok("flock-blocking");
    flock(fd, LOCK_UN);
    close(fd);
}

static void t_flock_badargs(void)
{
    errno = 0;
    if (flock(-1, LOCK_EX) == 0 || errno != EBADF) {
        bad("flock-badargs", "EBADF check errno=%d", errno);
        return;
    }
    int fd = open(LOCKFILE, O_RDWR);
    flock(fd, LOCK_UN);
    errno = 0;
    if (flock(fd, 0x40) == 0 || errno != EINVAL) {
        bad("flock-badargs", "EINVAL check errno=%d", errno);
        close(fd);
        return;
    }
    close(fd);
    ok("flock-badargs");
}

static void t_flock_indep_of_posix(void)
{
    int a = open(LOCKFILE, O_RDWR);
    if (a < 0) { bad("flock-indep-posix", "open errno=%d", errno); return; }
    if (flock(a, LOCK_EX) != 0) { bad("flock-indep-posix", "flock errno=%d", errno); close(a); return; }
    pid_t pid; int go;
    pid = locked_child2(cf_posix_wrlck_all, &go);
    /* POSIX write lock over the whole file must succeed under an flock */
    release_child(go, pid);

    /* and the converse: POSIX lock held, flock must succeed */
    pid = locked_child2(cf_wrlck_10_20, &go);
    if (flock(a, LOCK_EX | LOCK_NB) != 0) {
        bad("flock-indep-posix", "flock blocked by POSIX lock errno=%d", errno);
        release_child(go, pid);
        flock(a, LOCK_UN);
        close(a);
        return;
    }
    flock(a, LOCK_UN);
    release_child(go, pid);
    ok("flock-indep-posix");
    close(a);
}

/* ========================================================================= */
/* fcntl POSIX record-lock tests                                             */
/* ========================================================================= */

static void t_posix_self_no_conflict(void)
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0) { bad("posix-self-no-conflict", "open errno=%d", errno); return; }
    set_lock(fd, F_UNLCK, 0, 0);
    set_lock(fd, F_WRLCK, 10, 10);
    struct flock fl = get_first_conflict(fd, 10, 10);
    if (fl.l_type != F_UNLCK) { bad("posix-self-no-conflict", "own lock conflicts l_type=%d", fl.l_type); goto out; }
    /* overlapping re-set: replacement, never conflict */
    {
        struct flock f2 = { F_WRLCK, SEEK_SET, 15, 10, 0 };
        if (fcntl(fd, F_SETLK, &f2) != 0) { bad("posix-self-no-conflict", "overlap errno=%d", errno); goto out; }
    }
    ok("posix-self-no-conflict");
out:
    set_lock(fd, F_UNLCK, 0, 0);
    close(fd);
}

static void t_posix_conflict_and_boundaries(void)
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0) { bad("posix-conflict-boundaries", "open errno=%d", errno); return; }
    set_lock(fd, F_UNLCK, 0, 0);
    pid_t pid; int go;
    pid = locked_child2(cf_wrlck_10_20, &go);

    struct flock fl;
    errno = 0;
    fl = (struct flock){ F_WRLCK, SEEK_SET, 15, 10, 0 };
    if (fcntl(fd, F_SETLK, &fl) == 0 || errno != EAGAIN) {
        bad("posix-conflict-boundaries", "overlap W errno=%d", errno);
        goto out;
    }
    errno = 0;
    fl = (struct flock){ F_RDLCK, SEEK_SET, 15, 10, 0 };
    if (fcntl(fd, F_SETLK, &fl) == 0 || errno != EAGAIN) {
        bad("posix-conflict-boundaries", "overlap R errno=%d", errno);
        goto out;
    }
    fl = (struct flock){ F_WRLCK, SEEK_SET, 0, 10, 0 };   /* [0,10) touches */
    if (fcntl(fd, F_SETLK, &fl) != 0) { bad("posix-conflict-boundaries", "[0,10) errno=%d", errno); goto out; }
    fl = (struct flock){ F_WRLCK, SEEK_SET, 20, 10, 0 };  /* [20,30) touches */
    if (fcntl(fd, F_SETLK, &fl) != 0) { bad("posix-conflict-boundaries", "[20,30) errno=%d", errno); goto out; }
    ok("posix-conflict-boundaries");
out:
    set_lock(fd, F_UNLCK, 0, 0);
    release_child(go, pid);
    close(fd);
}

static void t_posix_getlk_backfill(void)
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0) { bad("posix-getlk-backfill", "open errno=%d", errno); return; }
    set_lock(fd, F_UNLCK, 0, 0);
    pid_t pid; int go;
    pid = locked_child2(cf_wrlck_10_20, &go);

    struct flock fl = get_first_conflict(fd, 12, 2);
    if (fl.l_type != F_WRLCK) { bad("posix-getlk-backfill", "l_type=%d", fl.l_type); goto out; }
    if (fl.l_start != 10 || fl.l_len != 10) {
        bad("posix-getlk-backfill", "l_start=%ld l_len=%ld want 10/10",
            (long)fl.l_start, (long)fl.l_len);
        goto out;
    }
    if (fl.l_pid != pid) { bad("posix-getlk-backfill", "l_pid=%d want %d", fl.l_pid, pid); goto out; }
    if (fl.l_whence != SEEK_SET) { bad("posix-getlk-backfill", "l_whence=%d", fl.l_whence); goto out; }
    fl = get_first_conflict(fd, 0, 10);
    if (fl.l_type != F_UNLCK) { bad("posix-getlk-backfill", "left boundary l_type=%d", fl.l_type); goto out; }
    fl = get_first_conflict(fd, 25, 5);
    if (fl.l_type != F_UNLCK) { bad("posix-getlk-backfill", "right boundary l_type=%d", fl.l_type); goto out; }
    ok("posix-getlk-backfill");
out:
    release_child(go, pid);
    close(fd);
}

static void t_posix_read_shared(void)
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0) { bad("posix-read-shared", "open errno=%d", errno); return; }
    set_lock(fd, F_UNLCK, 0, 0);
    pid_t pid; int go;
    pid = locked_child2(cf_rdlck_0_100, &go);

    struct flock fl = { F_RDLCK, SEEK_SET, 50, 100, 0 };
    if (fcntl(fd, F_SETLK, &fl) != 0) { bad("posix-read-shared", "R errno=%d", errno); goto out; }
    errno = 0;
    fl = (struct flock){ F_WRLCK, SEEK_SET, 50, 10, 0 };
    if (fcntl(fd, F_SETLK, &fl) == 0 || errno != EAGAIN) {
        bad("posix-read-shared", "W errno=%d", errno);
        goto out;
    }
    fl = get_first_conflict(fd, 60, 5);
    if (fl.l_type != F_RDLCK || fl.l_pid != pid) {
        bad("posix-read-shared", "GETLK type=%d pid=%d want RDLCK/%d", fl.l_type, fl.l_pid, pid);
        goto out;
    }
    ok("posix-read-shared");
out:
    set_lock(fd, F_UNLCK, 0, 0);
    release_child(go, pid);
    close(fd);
}

static void t_posix_split(void)
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0) { bad("posix-split", "open errno=%d", errno); return; }
    set_lock(fd, F_UNLCK, 0, 0);
    set_lock(fd, F_WRLCK, 0, 100);
    set_lock(fd, F_UNLCK, 40, 20);          /* punch hole [40,60) */

    struct flock fl;
    if (child_getlk(10, 20, &fl) != 0 ||    /* inside [0,40) */
        fl.l_type != F_WRLCK || fl.l_start != 0 || fl.l_len != 40) {
        bad("posix-split", "left frag type=%d start=%ld len=%ld want W/0/40",
            fl.l_type, (long)fl.l_start, (long)fl.l_len);
        goto out;
    }
    if (child_getlk(45, 10, &fl) != 0 ||    /* inside hole */
        fl.l_type != F_UNLCK) {
        bad("posix-split", "hole l_type=%d start=%ld len=%ld",
            fl.l_type, (long)fl.l_start, (long)fl.l_len);
        goto out;
    }
    if (child_getlk(70, 10, &fl) != 0 ||    /* inside [60,100) */
        fl.l_type != F_WRLCK || fl.l_start != 60 || fl.l_len != 40) {
        bad("posix-split", "right frag type=%d start=%ld len=%ld want W/60/40",
            fl.l_type, (long)fl.l_start, (long)fl.l_len);
        goto out;
    }
    ok("posix-split");
out:
    set_lock(fd, F_UNLCK, 0, 0);
    close(fd);
}


static void t_posix_merge(void)
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0) { bad("posix-merge", "open errno=%d", errno); return; }
    set_lock(fd, F_UNLCK, 0, 0);
    set_lock(fd, F_WRLCK, 0, 50);
    set_lock(fd, F_WRLCK, 50, 50);          /* adjacent: must coalesce */

    struct flock fl;
    if (child_getlk(70, 10, &fl) != 0 ||
        fl.l_type != F_WRLCK || fl.l_start != 0 || fl.l_len != 100) {
        bad("posix-merge", "type=%d start=%ld len=%ld want W/0/100",
            fl.l_type, (long)fl.l_start, (long)fl.l_len);
        goto out;
    }
    ok("posix-merge");
out:
    set_lock(fd, F_UNLCK, 0, 0);
    close(fd);
}

static void t_posix_replace_downgrade(void)
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0) { bad("posix-replace-downgrade", "open errno=%d", errno); return; }
    set_lock(fd, F_UNLCK, 0, 0);
    set_lock(fd, F_WRLCK, 0, 100);
    set_lock(fd, F_RDLCK, 40, 20);          /* downgrade own middle */

    struct flock fl;
    if (child_getlk(45, 5, &fl) != 0 ||
        fl.l_type != F_RDLCK || fl.l_start != 40 || fl.l_len != 20) {
        bad("posix-replace-downgrade", "middle type=%d start=%ld len=%ld want R/40/20",
            fl.l_type, (long)fl.l_start, (long)fl.l_len);
        goto out;
    }
    /* query only inside [0,40): must see W fragment */
    if (child_getlk(10, 20, &fl) != 0 ||
        fl.l_type != F_WRLCK || fl.l_start != 0 || fl.l_len != 40) {
        bad("posix-replace-downgrade", "left type=%d start=%ld len=%ld want W/0/40",
            fl.l_type, (long)fl.l_start, (long)fl.l_len);
        goto out;
    }
    ok("posix-replace-downgrade");
out:
    set_lock(fd, F_UNLCK, 0, 0);
    close(fd);
}


static void t_posix_close_any_fd(void)
{
    /* POSIX rule: closing ANY fd of the file drops ALL of the closing
     * process's record locks on that file. flock does NOT follow it. */
    int fd = open(LOCKFILE, O_RDWR);
    int fd2 = open(LOCKFILE, O_RDWR);
    if (fd < 0 || fd2 < 0) { bad("posix-close-any-fd", "open errno=%d", errno); return; }
    set_lock(fd, F_UNLCK, 0, 0);
    flock(fd2, LOCK_UN);

    set_lock(fd, F_WRLCK, 0, 100);          /* POSIX lock via fd */
    flock(fd2, LOCK_EX);                    /* flock on fd2 */
    close(fd);                              /* closes one fd of the file */

    /* 1) POSIX locks of this process must be gone (probe from a child:
     *    the parent's own GETLK could never see them anyway) */
    struct flock fl;
    if (child_getlk(50, 5, &fl) != 0 || fl.l_type != F_UNLCK) {
        bad("posix-close-any-fd", "record lock survived close(any fd) l_type=%d", fl.l_type);
        flock(fd2, LOCK_UN);
        close(fd2);
        return;
    }

    /* 2) flock must still be held: a child's fresh-open EX|NB fails */
    pid_t pid = fork();
    if (pid == 0) {
        int c = open(LOCKFILE, O_RDWR);
        _exit(flock(c, LOCK_EX | LOCK_NB) == 0 ? 0 : 1);
    }
    int st;
    waitpid(pid, &st, 0);
    flock(fd2, LOCK_UN);
    if (WIFEXITED(st) && WEXITSTATUS(st) == 0) {
        bad("posix-close-any-fd", "flock released by close of another fd");
        close(fd2);
        return;
    }
    ok("posix-close-any-fd");
    close(fd2);
}

static void t_posix_len0_eof(void)
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0) { bad("posix-len0-eof", "open errno=%d", errno); return; }
    set_lock(fd, F_UNLCK, 0, 0);
    struct flock fl = { F_WRLCK, SEEK_SET, 50, 0, 0 };   /* [50, EOF) */
    if (fcntl(fd, F_SETLK, &fl) != 0) { bad("posix-len0-eof", "set errno=%d", errno); goto out; }

    if (child_getlk(50, 10, &fl) != 0 ||
        fl.l_type != F_WRLCK || fl.l_start != 50) {
        bad("posix-len0-eof", "GETLK type=%d start=%ld want W/50", fl.l_type, (long)fl.l_start);
        goto out;
    }
    if (child_getlk(40, 10, &fl) != 0 ||    /* [40,50) touches only */
        fl.l_type != F_UNLCK) {
        bad("posix-len0-eof", "left boundary l_type=%d", fl.l_type);
        goto out;
    }
    /* lock extends to EOF: a child locking beyond current size conflicts */
    {
        pid_t pid = fork();
        if (pid == 0) {
            int c = open(LOCKFILE, O_RDWR);
            struct flock f2 = { F_WRLCK, SEEK_SET, 1000, 10, 0 };
            _exit(fcntl(c, F_SETLK, &f2) == 0 ? 0 : (errno == EAGAIN ? 1 : 2));
        }
        int st;
        waitpid(pid, &st, 0);
        if (!(WIFEXITED(st) && WEXITSTATUS(st) == 1)) {
            bad("posix-len0-eof", "beyond-EOF probe st=0x%x", st);
            goto out;
        }
    }
    ok("posix-len0-eof");
out:
    set_lock(fd, F_UNLCK, 0, 0);
    close(fd);
}

static void t_posix_whence_end(void)
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0) { bad("posix-whence-end", "open errno=%d", errno); return; }
    set_lock(fd, F_UNLCK, 0, 0);
    /* file is 100 bytes; lock [90,95) via SEEK_END-relative request */
    struct flock fl = { F_WRLCK, SEEK_END, -10, 5, 0 };
    if (fcntl(fd, F_SETLK, &fl) != 0) { bad("posix-whence-end", "set errno=%d", errno); goto out; }

    if (child_getlk(92, 2, &fl) != 0 ||
        fl.l_type != F_WRLCK || fl.l_start != 90 || fl.l_len != 5) {
        bad("posix-whence-end", "GETLK type=%d start=%ld len=%ld want W/90/5",
            fl.l_type, (long)fl.l_start, (long)fl.l_len);
        goto out;
    }
    if (child_getlk(95, 5, &fl) != 0 || fl.l_type != F_UNLCK) {
        bad("posix-whence-end", "right boundary l_type=%d", fl.l_type);
        goto out;
    }
    if (child_getlk(85, 5, &fl) != 0 || fl.l_type != F_UNLCK) {
        bad("posix-whence-end", "left boundary l_type=%d", fl.l_type);
        goto out;
    }
    ok("posix-whence-end");
out:
    set_lock(fd, F_UNLCK, 0, 0);
    close(fd);
}

static void t_posix_setlkw_block(void)
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0) { bad("posix-setlkw-block", "open errno=%d", errno); return; }
    set_lock(fd, F_UNLCK, 0, 0);
    pid_t pid = fork();
    if (pid == 0) _exit(cf_wrlck_10_20_1500() ? 1 : 0);
    usleep(300 * 1000);

    double t0 = now_s();
    struct flock fl = { F_WRLCK, SEEK_SET, 5, 20, 0 };
    int r = fcntl(fd, F_SETLKW, &fl);
    double dt = now_s() - t0;
    waitpid(pid, NULL, 0);
    if (r != 0) { bad("posix-setlkw-block", "errno=%d", errno); goto out; }
    if (dt < 0.5) { bad("posix-setlkw-block", "returned too early dt=%.2fs", dt); goto out; }
    if (dt > 30.0) { bad("posix-setlkw-block", "took too long dt=%.2fs", dt); goto out; }
    ok("posix-setlkw-block");
out:
    set_lock(fd, F_UNLCK, 0, 0);
    close(fd);
}

static void t_posix_setlkw_eintr(void)
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0) { bad("posix-setlkw-eintr", "open errno=%d", errno); return; }
    set_lock(fd, F_UNLCK, 0, 0);
    pid_t pid = fork();
    if (pid == 0) _exit(cf_wrlck_0_10_3000() ? 1 : 0);
    usleep(300 * 1000);

    /* killer: signals us after ~800ms, handler has NO SA_RESTART */
    pid_t killer = fork();
    if (killer == 0) {
        usleep(800 * 1000);
        kill(getppid(), SIGUSR1);
        _exit(0);
    }
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = sig_handler;            /* no SA_RESTART */
    sigemptyset(&sa.sa_mask);
    sigaction(SIGUSR1, &sa, NULL);
    g_got_sig = 0;

    double t0 = now_s();
    struct flock fl = { F_WRLCK, SEEK_SET, 0, 10, 0 };
    int r = fcntl(fd, F_SETLKW, &fl);
    double dt = now_s() - t0;
    if (r == 0) { bad("posix-setlkw-eintr", "granted without conflict dt=%.2f", dt); goto out; }
    if (errno != EINTR) { bad("posix-setlkw-eintr", "errno=%d want EINTR(%d)", errno, EINTR); goto out; }
    if (!g_got_sig) { bad("posix-setlkw-eintr", "no signal seen"); goto out; }
    if (dt > 30.0) { bad("posix-setlkw-eintr", "took too long dt=%.2f", dt); goto out; }
    ok("posix-setlkw-eintr");
out:
    waitpid(killer, NULL, 0);
    waitpid(pid, NULL, 0);
    set_lock(fd, F_UNLCK, 0, 0);
    close(fd);
}

static void t_posix_access_mode(void)
{
    int ro = open(LOCKFILE, O_RDONLY);
    int wo = open(LOCKFILE, O_WRONLY);
    if (ro < 0 || wo < 0) { bad("posix-access-mode", "open errno=%d", errno); return; }
    set_lock(ro, F_UNLCK, 0, 0);

    struct flock fl = { F_WRLCK, SEEK_SET, 0, 10, 0 };
    errno = 0;
    if (fcntl(ro, F_SETLK, &fl) == 0 || errno != EBADF) {
        bad("posix-access-mode", "W on O_RDONLY errno=%d want EBADF", errno);
        goto out;
    }
    fl = (struct flock){ F_RDLCK, SEEK_SET, 0, 10, 0 };
    if (fcntl(ro, F_SETLK, &fl) != 0) { bad("posix-access-mode", "R on O_RDONLY errno=%d", errno); goto out; }
    set_lock(ro, F_UNLCK, 0, 0);

    fl = (struct flock){ F_RDLCK, SEEK_SET, 0, 10, 0 };
    errno = 0;
    if (fcntl(wo, F_SETLK, &fl) == 0 || errno != EBADF) {
        bad("posix-access-mode", "R on O_WRONLY errno=%d want EBADF", errno);
        goto out;
    }
    fl = (struct flock){ F_WRLCK, SEEK_SET, 0, 10, 0 };
    if (fcntl(wo, F_SETLK, &fl) != 0) { bad("posix-access-mode", "W on O_WRONLY errno=%d", errno); goto out; }
    set_lock(wo, F_UNLCK, 0, 0);
    ok("posix-access-mode");
out:
    set_lock(ro, F_UNLCK, 0, 0);
    set_lock(wo, F_UNLCK, 0, 0);
    close(ro); close(wo);
}

static void t_posix_fork_not_inherited(void)
{
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0) { bad("posix-fork-not-inherited", "open errno=%d", errno); return; }
    set_lock(fd, F_UNLCK, 0, 0);
    set_lock(fd, F_WRLCK, 0, 10);

    /* child probes: lock is visible to the child but owned by the parent */
    int go[2];
    if (pipe(go) != 0) _exit(96);
    pid_t pid = fork();
    if (pid == 0) {
        close(go[1]);
        struct flock fl = { F_WRLCK, SEEK_SET, 0, 10, 0 };
        if (fcntl(fd, F_GETLK, &fl) != 0) _exit(1);
        if (fl.l_type != F_WRLCK || fl.l_pid != getppid()) _exit(2);
        /* child's own SETLK conflicts with the parent's lock */
        errno = 0;
        if (fcntl(fd, F_SETLK, &fl) == 0 || errno != EAGAIN) _exit(3);
        /* child's UNLOCK is a no-op for itself and must NOT free the
         * parent's lock (verified by the parent below) */
        fl = (struct flock){ F_UNLCK, SEEK_SET, 0, 10, 0 };
        if (fcntl(fd, F_SETLK, &fl) != 0) _exit(4);
        char g;
        if (read(go[0], &g, 1) < 0) _exit(96);
        _exit(0);
    }
    close(go[1]);
    int st;
    waitpid(pid, &st, 0);
    if (!(WIFEXITED(st) && WEXITSTATUS(st) == 0)) {
        bad("posix-fork-not-inherited", "child st=0x%x", st);
        goto out;
    }
    /* parent's lock must have survived the child's UNLOCK — verified
     * from a fresh child (the parent's own GETLK never sees its locks) */
    pid_t pid2 = fork();
    if (pid2 == 0) {
        int probe = open(LOCKFILE, O_RDWR);
        struct flock fl = { F_WRLCK, SEEK_SET, 5, 5, 0 };
        if (fcntl(probe, F_GETLK, &fl) != 0) _exit(1);
        if (fl.l_type != F_WRLCK || fl.l_pid != getppid()) _exit(2);
        _exit(0);
    }
    int st2;
    waitpid(pid2, &st2, 0);
    if (!(WIFEXITED(st2) && WEXITSTATUS(st2) == 0)) {
        bad("posix-fork-not-inherited", "parent lock lost after child UNLOCK st=0x%x", st2);
        goto out;
    }
    ok("posix-fork-not-inherited");
out:
    set_lock(fd, F_UNLCK, 0, 0);
    close(fd);
}

static void t_posix_exit_releases(void)
{
    /* child holds W [10,20) and _exits while holding it */
    int fd = open(LOCKFILE, O_RDWR);
    if (fd < 0) { bad("posix-exit-releases", "open errno=%d", errno); return; }
    set_lock(fd, F_UNLCK, 0, 0);

    pid_t pid = fork();
    if (pid == 0) {
        int c = open(LOCKFILE, O_RDWR);
        struct flock fl = { F_WRLCK, SEEK_SET, 10, 10, 0 };
        if (fcntl(c, F_SETLK, &fl) != 0) _exit(1);
        _exit(0);                            /* exits holding the lock */
    }
    waitpid(pid, NULL, 0);
    usleep(200 * 1000);                     /* give reaping a moment */

    struct flock fl = { F_WRLCK, SEEK_SET, 10, 10, 0 };
    if (fcntl(fd, F_SETLK, &fl) != 0) {
        bad("posix-exit-releases", "lock survived owner exit errno=%d", errno);
        goto out;
    }
    ok("posix-exit-releases");
out:
    set_lock(fd, F_UNLCK, 0, 0);
    close(fd);
}

/* ========================================================================= */

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);

    /* Reference runs outside init can point LOCKFILE elsewhere. */
    {
        const char *env = getenv("LOCKFILE");
        if (env && *env) LOCKFILE = env;
    }

    /* fresh 100-byte lock file */
    int fd = open(LOCKFILE, O_RDWR | O_CREAT | O_TRUNC, 0644);
    if (fd >= 0) {
        char buf[100];
        memset(buf, 'x', sizeof buf);
        if (write(fd, buf, sizeof buf) != (ssize_t)sizeof buf)
            printf("WARN cannot size %s\n", LOCKFILE);
        close(fd);
    }

    t_flock_basic();
    t_flock_conflict_two_opens();
    t_flock_sh_shared_and_upgrade();
    t_flock_dup_shares();
    t_flock_fork();
    t_flock_release_on_close();
    t_flock_blocking();
    t_flock_badargs();
    t_flock_indep_of_posix();

    t_posix_self_no_conflict();
    t_posix_conflict_and_boundaries();
    t_posix_getlk_backfill();
    t_posix_read_shared();
    t_posix_split();
    t_posix_merge();
    t_posix_replace_downgrade();
    t_posix_close_any_fd();
    t_posix_len0_eof();
    t_posix_whence_end();
    t_posix_setlkw_block();
    t_posix_setlkw_eintr();
    t_posix_access_mode();
    t_posix_fork_not_inherited();
    t_posix_exit_releases();

    printf("PROBE-SUMMARY pass=%d fail=%d\n", g_pass, g_fail);
    printf("PROBE-DONE\n");

    if (getpid() == 1) {
        for (;;) sleep(60);                 /* park init */
    }
    return g_fail ? 1 : 0;
}
