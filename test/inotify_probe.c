// inotify_probe.c — static verification probe for inotify(7) (P0-4).
//
// Boots as init (or runs from a shell) and asserts the full Linux-visible
// contract of the three inotify syscalls:
//
//   A. Directory watch (IN_ALL_EVENTS): exact event sequence for
//      open(O_CREAT)+write+close+rename+unlink+mkdir+rmdir, each event
//      tagged with the entry name; IN_ISDIR on directory subjects;
//      IN_MOVED_FROM/IN_MOVED_TO share a nonzero cookie.
//   B. File watch: name-less IN_OPEN/IN_ACCESS/IN_MODIFY/IN_CLOSE_WRITE/
//      IN_CLOSE_NOWRITE/IN_ATTRIB on the file's own wd; IN_MASK_ADD
//      accumulates masks on a re-add.
//   C. Blocking read: a forked child creates a file after 300 ms; the
//      parent's read must block until the event arrives (elapsed >= 100ms).
//   D. Error/flag paths: IN_NONBLOCK read -> EAGAIN, IN_CLOEXEC -> F_SETFD,
//      EINVAL for bad flags/mask/wd/non-inotify fd, ENOENT for a missing
//      path, rm_watch queues IN_IGNORED, re-add returns a fresh wd.
//   E. IN_MOVE_SELF / IN_DELETE_SELF + IN_IGNORED when the watched
//      directory itself is renamed/removed.
//   F. Buffer semantics: 8-byte buffer -> EINVAL (smaller than an event
//      header); a named event that cannot fit -> EINVAL, not a short read.
//
// Prints one "ok"/"FAIL" line per check and a final INOTEST-PASS /
// INOTEST-FAIL summary; exit code = number of failed checks.

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/inotify.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <sys/wait.h>

#ifndef SYS_inotify_init1
#define SYS_inotify_init1 26
#endif

#ifndef INOTEST_DEFAULT_BASE
#define INOTEST_DEFAULT_BASE "/tmp/inop"
#endif

static int g_fail;
static int g_ok;

static void ok(const char *what) {
    g_ok++;
    printf("ok       %s\n", what);
}

static void fail(const char *what, const char *detail) {
    g_fail++;
    printf("FAIL     %s (%s)\n", what, detail ? detail : "");
}

static long sys_iinit1(uint32_t flags) {
    return syscall(SYS_inotify_init1, flags, 0, 0, 0, 0, 0);
}

// ---------------------------------------------------------------------------
// Event plumbing
// ---------------------------------------------------------------------------

struct ev {
    int      wd;
    uint32_t mask;
    uint32_t cookie;
    uint32_t len;
    char     name[64];
};

#define MAXEV 64
static struct ev evs[MAXEV];
static int nev;

static void drain(int fd) {
    static char buf[16384];
    // An empty inotify queue BLOCKS in read(); temporarily go nonblocking,
    // then restore the previous mode (phase C relies on blocking reads).
    int oflags = fcntl(fd, F_GETFL);
    fcntl(fd, F_SETFL, oflags | O_NONBLOCK);
    nev = 0;
    for (;;) {
        int n = (int)read(fd, buf, sizeof buf);
        if (n <= 0)
            break;
        char *p = buf;
        while (p < buf + n && nev < MAXEV) {
            struct inotify_event *ie = (struct inotify_event *)p;
            struct ev *e = &evs[nev++];
            e->wd = ie->wd;
            e->mask = ie->mask;
            e->cookie = ie->cookie;
            e->len = ie->len;
            e->name[0] = 0;
            if (ie->len > 0) {
                int cl = ie->len < 63 ? ie->len : 63;
                memcpy(e->name, ie->name, cl);
                e->name[cl] = 0;
                // the name must be NUL-terminated inside ie->len
                if (memchr(ie->name, 0, ie->len) == NULL) {
                    fail("name NUL-terminated", "name field lacks NUL");
                }
            }
            p += sizeof *ie + ie->len;
        }
    }
    fcntl(fd, F_SETFL, oflags);
}

// Expected-mask helper: mask equality ignoring nothing (exact match).
static int is(struct ev *e, int wd, uint32_t mask, const char *name) {
    if (e->wd != wd || e->mask != mask)
        return 0;
    if (name == NULL)
        return e->len == 0 && e->name[0] == 0;
    return strcmp(e->name, name) == 0;
}

static void dump_expected(int i, int wd, uint32_t mask, const char *name) {
    printf("         ev[%d] got wd=%d mask=%08x cookie=%u name='%s'"
           " want wd=%d mask=%08x name='%s'\n",
           i, evs[i].wd, evs[i].mask, evs[i].cookie, evs[i].name,
           wd, mask, name ? name : "");
}

static void expect_ev(const char *tag, int i, int wd, uint32_t mask, const char *name) {
    char lbl[128];
    snprintf(lbl, sizeof lbl, "%s ev[%d]", tag, i);
    if (i >= nev) {
        char d[64];
        snprintf(d, sizeof d, "missing (only %d events)", nev);
        fail(lbl, d);
        return;
    }
    if (!is(&evs[i], wd, mask, name)) {
        dump_expected(i, wd, mask, name);
        fail(lbl, "mismatch");
        return;
    }
    ok(lbl);
}

static double now_ms(void) {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return tv.tv_sec * 1000.0 + tv.tv_usec / 1000.0;
}

// ---------------------------------------------------------------------------
int main(int argc, char **argv)
{
    // Base directory override: argv[1], else the compile-time default
    // (pass -DINOTEST_DEFAULT_BASE=\"/dev/shm/inop\" to target tmpfs).
    const char *base = argc > 1 ? argv[1] : INOTEST_DEFAULT_BASE;
    const char *q = "/tmp/inoq";
    const char *q2 = "/tmp/inoq2";
    char path[256];
    setvbuf(stdout, NULL, _IONBF, 0);

    // -------- setup -------------------------------------------------------
    mkdir(q, 0755); // in case phase E leftovers exist
    rmdir(q2);
    if (mkdir(base, 0755) != 0 && errno != EEXIST) {
        fail("mkdir base", strerror(errno));
        return 1;
    }
    // start from an empty directory (ignore failures)
    {
        const char *junk[] = {"a", "b", "f", "late", "sub", "bigname", NULL};
        for (int i = 0; junk[i]; i++) {
            snprintf(path, sizeof path, "%s/%s", base, junk[i]);
            unlink(path);
            rmdir(path);
        }
    }

    // -------- A: directory watch, exact create/modify/close/rename/delete --
    int fd = (int)sys_iinit1(0);
    if (fd < 3) { fail("inotify_init1", strerror(errno)); return 1; }
    ok("inotify_init1 returns fd");

    int wd = inotify_add_watch(fd, base, IN_ALL_EVENTS);
    if (wd != 1) {
        char d[64];
        snprintf(d, sizeof d, "wd=%d errno=%s", wd, strerror(errno));
        fail("add_watch first wd == 1", d);
    } else {
        ok("add_watch first wd == 1");
    }

    int fa = open("/dev/null", O_RDONLY); (void)fa; // fd noise guard

    snprintf(path, sizeof path, "%s/a", base);
    int wfd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (wfd < 0) { fail("open a O_CREAT", strerror(errno)); return 1; }
    if (write(wfd, "hello", 5) != 5) { fail("write a", strerror(errno)); }
    close(wfd);
    snprintf(path, sizeof path, "%s/b", base);
    char oldpath[256];
    snprintf(oldpath, sizeof oldpath, "%s/a", base);
    if (rename(oldpath, path) != 0) { fail("rename a->b", strerror(errno)); }
    if (unlink(path) != 0) { fail("unlink b", strerror(errno)); }
    snprintf(path, sizeof path, "%s/sub", base);
    if (mkdir(path, 0755) != 0) { fail("mkdir sub", strerror(errno)); }
    if (rmdir(path) != 0) { fail("rmdir sub", strerror(errno)); }

    usleep(100000); // let any kernel-side deferred work land
    drain(fd);
    expect_ev("A", 0, wd, IN_CREATE, "a");
    expect_ev("A", 1, wd, IN_OPEN, "a");
    expect_ev("A", 2, wd, IN_MODIFY, "a");
    expect_ev("A", 3, wd, IN_CLOSE_WRITE, "a");
    expect_ev("A", 4, wd, IN_MOVED_FROM, "a");
    expect_ev("A", 5, wd, IN_MOVED_TO, "b");
    expect_ev("A", 6, wd, IN_DELETE, "b");
    expect_ev("A", 7, wd, IN_CREATE | IN_ISDIR, "sub");
    expect_ev("A", 8, wd, IN_DELETE | IN_ISDIR, "sub");
    if (nev != 9) {
        char d[64];
        snprintf(d, sizeof d, "%d events (want 9)", nev);
        fail("A event count", d);
    } else {
        ok("A event count == 9");
    }
    if (nev >= 6 && evs[4].cookie != 0 && evs[4].cookie == evs[5].cookie) {
        ok("A rename cookie pairs MOVED_FROM/MOVED_TO");
    } else {
        fail("A rename cookie pairs MOVED_FROM/MOVED_TO", "cookie zero or mismatched");
    }

    // -------- B: file watch (own instance) --------------------------------
    snprintf(path, sizeof path, "%s/f", base);
    int ff = open(path, O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (ff < 0) { fail("create f", strerror(errno)); return 1; }
    if (write(ff, "0123456789", 10) != 10) { fail("write f", strerror(errno)); }
    close(ff);
    drain(fd); // drop IN_CREATE/IN_OPEN/IN_MODIFY/IN_CLOSE_WRITE(f) on wd

    int fd2 = (int)sys_iinit1(0);
    if (fd2 < 3) { fail("init1 #2", strerror(errno)); return 1; }
    int fw = inotify_add_watch(fd2, path, IN_ALL_EVENTS);
    if (fw < 1) { fail("add_watch file", strerror(errno)); return 1; }
    ok("add_watch on regular file");

    int rfd = open(path, O_RDONLY);
    char rb[8];
    if (read(rfd, rb, 4) != 4) { fail("read f", strerror(errno)); }
    close(rfd);
    int wfd2 = open(path, O_WRONLY);
    if (write(wfd2, "xy", 2) != 2) { fail("write f again", strerror(errno)); }
    close(wfd2);
    if (chmod(path, 0600) != 0) { fail("chmod f", strerror(errno)); }

    usleep(100000);
    drain(fd2);
    expect_ev("B", 0, fw, IN_OPEN, NULL);
    expect_ev("B", 1, fw, IN_ACCESS, NULL);
    expect_ev("B", 2, fw, IN_CLOSE_NOWRITE, NULL);
    expect_ev("B", 3, fw, IN_OPEN, NULL);
    expect_ev("B", 4, fw, IN_MODIFY, NULL);
    expect_ev("B", 5, fw, IN_CLOSE_WRITE, NULL);
    expect_ev("B", 6, fw, IN_ATTRIB, NULL);
    if (nev != 7) {
        char d[64];
        snprintf(d, sizeof d, "%d events (want 7)", nev);
        fail("B event count", d);
    } else {
        ok("B event count == 7");
    }

    // -------- B2: IN_MASK_ADD accumulates ---------------------------------
    int fw2 = inotify_add_watch(fd2, path, IN_CREATE);        // replaces -> CREATE only
    if (fw2 != fw) { fail("B2 re-add keeps wd", "wd changed"); }
    else ok("B2 re-add of same inode keeps wd");
    inotify_add_watch(fd2, path, IN_MODIFY | IN_MASK_ADD);    // -> CREATE|MODIFY
    drain(fd2);                                                // eat IN_IGNORED? no: re-add does not queue
    int wfd3 = open(path, O_WRONLY);
    if (write(wfd3, "z", 1) != 1) { fail("write f z", strerror(errno)); }
    close(wfd3);
    unlink(path); // IN_DELETE goes to the dir watch; file watch: DELETE_SELF
    usleep(100000);
    drain(fd2);
    // With mask CREATE|MODIFY the file watch sees: OPEN? no (not in mask),
    // MODIFY (nameless). DELETE_SELF is NOT in the mask so it is filtered;
    // the watch is still auto-removed when the inode dies -> IN_IGNORED.
    expect_ev("B2", 0, fw, IN_MODIFY, NULL);
    expect_ev("B2", 1, fw, IN_IGNORED, NULL);
    if (nev == 2)
        ok("B2 IN_MASK_ADD accumulates (MODIFY delivered, OPEN filtered)");
    else {
        char d[96];
        snprintf(d, sizeof d, "nev=%d (want 2: MODIFY + IN_IGNORED)", nev);
        fail("B2 IN_MASK_ADD accumulates", d);
    }

    // -------- C: blocking read is woken by the event ----------------------
    drain(fd); // flush the IN_DELETE(f) the unlink above left on the dir watch
    pid_t pid = fork();
    if (pid == 0) {
        usleep(300000);
        snprintf(path, sizeof path, "%s/late", base);
        int cfd = open(path, O_CREAT | O_WRONLY, 0644);
        if (cfd >= 0) close(cfd);
        _exit(0);
    }
    double t0 = now_ms();
    char cbuf[4096];
    int cn = (int)read(fd, cbuf, sizeof cbuf);
    double dt = now_ms() - t0;
    int cst;
    waitpid(pid, &cst, 0);
    if (cn > 0 && dt >= 100.0) {
        ok("C blocking read waits for the event");
    } else {
        char d[96];
        snprintf(d, sizeof d, "n=%d dt=%.1fms errno=%s", cn, dt, strerror(errno));
        fail("C blocking read waits for the event", d);
    }
    // first event of the blocking read must be IN_CREATE "late"
    if (cn >= (int)sizeof(struct inotify_event)) {
        struct inotify_event *ie = (struct inotify_event *)cbuf;
        if (ie->wd == wd && ie->mask == IN_CREATE && ie->len > 0 &&
            strcmp(ie->name, "late") == 0)
            ok("C first woken event is IN_CREATE 'late'");
        else {
            char d[96];
            snprintf(d, sizeof d, "wd=%d mask=%08x name='%.*s'",
                     ie->wd, ie->mask, (int)(ie->len ? ie->len - 1 : 0), ie->name);
            fail("C first woken event is IN_CREATE 'late'", d);
        }
    } else {
        fail("C first woken event is IN_CREATE 'late'", "short read");
    }
    drain(fd); // drop the remaining OPEN/CLOSE_WRITE(late) records
    ok("C child exit reaped");

    // -------- D: flags & error paths ---------------------------------------
    int fd3 = (int)sys_iinit1(IN_NONBLOCK);
    char b8[8];
    errno = 0;
    int rn = (int)read(fd3, b8, sizeof b8);
    if (rn == -1 && errno == EAGAIN) ok("D IN_NONBLOCK empty read -> EAGAIN");
    else { char d[64]; snprintf(d, sizeof d, "n=%d errno=%d", rn, errno);
           fail("D IN_NONBLOCK empty read -> EAGAIN", d); }

    int fd4 = (int)sys_iinit1(IN_CLOEXEC);
    int fdf = fcntl(fd4, F_GETFD);
    if (fdf & FD_CLOEXEC) ok("D IN_CLOEXEC sets FD_CLOEXEC");
    else fail("D IN_CLOEXEC sets FD_CLOEXEC", "FD_CLOEXEC clear");

    errno = 0;
    long r = sys_iinit1(0x10000);
    if (r == -1 && errno == EINVAL) ok("D bad init flags -> EINVAL");
    else { fail("D bad init flags -> EINVAL", "accepted"); }

    errno = 0;
    r = inotify_add_watch(fd3, "/no/such/path/xyz", IN_ALL_EVENTS);
    if (r == -1 && errno == ENOENT) ok("D add_watch missing path -> ENOENT");
    else { char d[64]; snprintf(d, sizeof d, "r=%ld errno=%d", r, errno);
           fail("D add_watch missing path -> ENOENT", d); }

    errno = 0;
    r = inotify_add_watch(fd3, base, 0);
    if (r == -1 && errno == EINVAL) ok("D add_watch mask 0 -> EINVAL");
    else fail("D add_watch mask 0 -> EINVAL", "accepted");

    errno = 0;
    r = inotify_add_watch(1, base, IN_ALL_EVENTS); // stdout is not inotify
    if (r == -1 && errno == EINVAL) ok("D add_watch non-inotify fd -> EINVAL");
    else { char d[64]; snprintf(d, sizeof d, "r=%ld errno=%d", r, errno);
           fail("D add_watch non-inotify fd -> EINVAL", d); }

    errno = 0;
    r = inotify_rm_watch(fd3, 4242);
    if (r == -1 && errno == EINVAL) ok("D rm_watch unknown wd -> EINVAL");
    else fail("D rm_watch unknown wd -> EINVAL", "accepted");

    int w3 = inotify_add_watch(fd3, base, IN_ALL_EVENTS);
    errno = 0;
    r = inotify_rm_watch(fd3, w3);
    if (r == 0) ok("D rm_watch valid wd -> 0");
    else fail("D rm_watch valid wd -> 0", strerror(errno));
    usleep(50000);
    drain(fd3);
    if (nev == 1 && is(&evs[0], w3, IN_IGNORED, NULL))
        ok("D rm_watch queues IN_IGNORED");
    else { char d[96];
           snprintf(d, sizeof d, "nev=%d ev0 mask=%08x", nev, nev ? evs[0].mask : 0);
           fail("D rm_watch queues IN_IGNORED", d); }
    int w3b = inotify_add_watch(fd3, base, IN_ALL_EVENTS);
    if (w3b > w3) ok("D re-add after rm returns fresh wd");
    else { char d[64]; snprintf(d, sizeof d, "old=%d new=%d", w3, w3b);
           fail("D re-add after rm returns fresh wd", d); }

    // -------- E: MOVE_SELF / DELETE_SELF on the watched dir ---------------
    // NOTE: Linux does NOT set IN_ISDIR on the *_SELF events (it is only
    // for the parent-directory entry events).
    rmdir(q); rmdir(q2);
    if (mkdir(q, 0755) != 0) { fail("E mkdir q", strerror(errno)); return 1; }
    int fd5 = (int)sys_iinit1(0);
    int wq = inotify_add_watch(fd5, q, IN_ALL_EVENTS);
    if (rename(q, q2) != 0) { fail("E rename q", strerror(errno)); }
    if (rmdir(q2) != 0) { fail("E rmdir q2", strerror(errno)); }
    usleep(100000);
    drain(fd5);
    expect_ev("E", 0, wq, IN_MOVE_SELF, NULL);
    expect_ev("E", 1, wq, IN_DELETE_SELF, NULL);
    expect_ev("E", 2, wq, IN_IGNORED, NULL);
    if (nev == 3) ok("E event count == 3");
    else { char d[64]; snprintf(d, sizeof d, "%d events (want 3)", nev);
           fail("E event count == 3", d); }

    // -------- F: buffer-size semantics --------------------------------------
    // fd3 is IN_NONBLOCK and holds no events here; use a fresh blocking fd.
    {
        int fdf = (int)sys_iinit1(0);
        inotify_add_watch(fdf, base, IN_CREATE);
        snprintf(path, sizeof path, "%s/bigname", base);
        int bf = open(path, O_CREAT | O_WRONLY, 0644);
        int w4 = -1;
        if (bf >= 0) close(bf);
        usleep(100000);

        char tiny[8]; // smaller than the event header itself
        errno = 0;
        int n = (int)read(fdf, tiny, sizeof tiny);
        if (n == -1 && errno == EINVAL) ok("F buffer < header -> EINVAL");
        else { char d[64]; snprintf(d, sizeof d, "n=%d errno=%d", n, errno);
               fail("F buffer < header -> EINVAL", d); }

        char small[16]; // header fits, the name does not
        errno = 0;
        n = (int)read(fdf, small, sizeof small);
        if (n == -1 && errno == EINVAL) ok("F unfittable named event -> EINVAL");
        else { char d[64]; snprintf(d, sizeof d, "n=%d errno=%d", n, errno);
               fail("F unfittable named event -> EINVAL", d); }

        drain(fdf); // now drain it for real (restores blocking mode)
        // locate our IN_CREATE(bigname): an IN_IGNORED for the auto-removed
        // watch may trail it after rm-by-not-needed; just require presence.
        int found = 0;
        for (int i = 0; i < nev && !found; i++)
            found = is(&evs[i], evs[i].wd, IN_CREATE, "bigname") &&
                    evs[i].wd >= 1 && evs[i].name[0] == 'b';
        if (found)
            ok("F event intact after failed small reads");
        else fail("F event intact after failed small reads", "lost or mangled");
        unlink(path);
        close(fdf);
        (void)w4;
    }

    // -------- G: truncate semantics ----------------------------------------
    // Linux reference (verified against 6.x): every truncate/ftruncate of
    // an EXISTING inode fires IN_MODIFY (never IN_ATTRIB), even when the
    // size does not change; the only silent O_TRUNC is the one riding on
    // an open that just CREATED the file. Linux coalesces consecutive
    // identical events, so MODIFY runs are asserted as ranges.
    {
        snprintf(path, sizeof path, "%s/g", base);
        int gf = open(path, O_CREAT | O_RDWR | O_TRUNC, 0644);
        if (write(gf, "0123456789", 10) != 10) { fail("G write g", ""); }
        close(gf);
        int fdg = (int)sys_iinit1(0);
        int gw = inotify_add_watch(fdg, path, IN_ALL_EVENTS);

        int w5 = open(path, O_WRONLY | O_TRUNC);          // existing non-empty
        close(w5);
        int w6 = open(path, O_WRONLY);                    // now empty
        if (write(w6, "abcde", 5) != 5) {}
        ftruncate(w6, 5);                                 // size no-op: still MODIFY
        ftruncate(w6, 2);                                 // shrink
        close(w6);
        truncate(path, 4);                                // grow
        truncate(path, 0);                                // shrink
        int w7 = open(path, O_WRONLY | O_TRUNC);          // existing empty: MODIFY
        close(w7);
        usleep(100000);
        drain(fdg);

        // mandatory skeleton (MODIFY runs as ranges for coalescing)
        expect_ev("G", 0, gw, IN_OPEN, NULL);
        expect_ev("G", 1, gw, IN_MODIFY, NULL);           // O_TRUNC non-empty
        expect_ev("G", 2, gw, IN_CLOSE_WRITE, NULL);
        expect_ev("G", 3, gw, IN_OPEN, NULL);
        int i = 4, m1 = 0;
        while (i < nev && evs[i].mask == IN_MODIFY) { m1++; i++; }
        if (m1 >= 1 && m1 <= 3) ok("G write+ftruncates -> 1-3 MODIFY");
        else { char d[48]; snprintf(d, sizeof d, "m1=%d", m1);
               fail("G write+ftruncates -> 1-3 MODIFY", d); }
        if (i < nev && is(&evs[i], gw, IN_CLOSE_WRITE, NULL)) ok("G close after modifies");
        else fail("G close after modifies", "missing");
        i++;
        int m2 = 0;
        while (i < nev && evs[i].mask == IN_MODIFY) { m2++; i++; }
        if (m2 >= 1 && m2 <= 2) ok("G truncate(2) x2 -> 1-2 MODIFY");
        else { char d[48]; snprintf(d, sizeof d, "m2=%d", m2);
               fail("G truncate(2) x2 -> 1-2 MODIFY", d); }
        if (i < nev && is(&evs[i], gw, IN_OPEN, NULL)) ok("G final open seen");
        else fail("G final open seen", "missing");
        i++;
        int m3 = 0;
        while (i < nev && evs[i].mask == IN_MODIFY) { m3++; i++; }
        if (m3 == 1) ok("G O_TRUNC on existing empty file -> 1 MODIFY");
        else { char d[48]; snprintf(d, sizeof d, "m3=%d", m3);
               fail("G O_TRUNC on existing empty file -> 1 MODIFY", d); }
        if (i < nev && is(&evs[i], gw, IN_CLOSE_WRITE, NULL) && i + 1 == nev)
            ok("G tail is CLOSE_WRITE");
        else { char d[96];
               snprintf(d, sizeof d, "tail: i=%d nev=%d", i, nev);
               fail("G tail is CLOSE_WRITE", d); }
        // no IN_ATTRIB anywhere
        int attr = 0;
        for (int k = 0; k < nev; k++) if (evs[k].mask & IN_ATTRIB) attr++;
        if (!attr) ok("G no IN_ATTRIB from truncates");
        else fail("G no IN_ATTRIB from truncates", "IN_ATTRIB seen");
        unlink(path);
        close(fdg);
    }

    // -------- cleanup & summary --------------------------------------------
    close(fd); close(fd2); close(fd3); close(fd4); close(fd5);
    snprintf(path, sizeof path, "%s/late", base);
    unlink(path);
    rmdir(base);

    printf("INOTEST: %d passed, %d failed\n", g_ok, g_fail);
    printf(g_fail == 0 ? "INOTEST-PASS\n" : "INOTEST-FAIL\n");
    return g_fail;
}
