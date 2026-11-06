/* spinprobe.c — trim-verdict lane: name the GNOME boot-park spin driver.
 *
 * The park face: gnome-session-check-accelerated (pid 318) spins at 100%
 * of one CPU in userspace (taskdump: vruntime +21.6s per 30s window,
 * state RUNNING on_cpu=1) while its GL helper child sits ZOMBIE unreaped.
 * A glib main loop spins at 100% when a source is perpetually ready but
 * its syscall makes no progress. Two kernel surfaces can produce that:
 *   H1: waitpid(WNOHANG) on a ZOMBIE child returns 0 forever (glib
 *       GChildWatch re-arms on 0 — spin, zombie never reaped).
 *   H2: poll() reports readiness on an empty socket/pipe (phantom-ready
 *       — a dispatch loop spins on EAGAIN reads).
 * Each check prints PASS (kernel behaved like the reference) or the
 * failure count. PROBE DONE ends the boot.
 */
#include <stdio.h>
#include <unistd.h>
#include <sys/wait.h>
#include <sys/socket.h>
#include <sys/resource.h>
#include <poll.h>
#include <string.h>
#include <errno.h>
#include <sys/un.h>

int main(void) {
    /* H1: child exits immediately; parent polls WNOHANG. A zombie child
     * must be reaped on the FIRST waitpid — returning 0 for a zombie is
     * the glib child-watch spin. Run the loop 3x (fresh pid each time)
     * and also with a mature zombie (100ms after exit). */
    for (int round = 0; round < 3; round++) {
        pid_t p = fork();
        if (p == 0) _exit(42 + round);
        int st = 0;
        int zeros = 0, reaped = 0, errs = 0;
        pid_t r = 0;
        for (int i = 0; i < 200000; i++) {
            r = waitpid(p, &st, WNOHANG);
            if (r == p) { reaped = 1; break; }
            if (r == 0) zeros++;
            else { errs++; printf("H1 waitpid errno=%s\n", strerror(errno)); break; }
        }
        printf("H1 round%d fork+exit waitpid(WNOHANG): reaped=%d zeros=%d errs=%d st=%d\n",
               round, reaped, zeros, errs, reaped ? st : -1);
        usleep(100000);
        /* mature zombie must already be reaped by now; if not, the
         * waitpid above silently failed — catch it here */
        r = waitpid(p, &st, WNOHANG);
        printf("H1 round%d post-100ms reap: r=%d errno=%d st=%d\n",
               round, (int)r, (int)errno, r == p ? st : -1);
    }

    /* H2: poll on an EMPTY unix socketpair end must block-timeout (0),
     * never report POLLIN. */
    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) != 0) {
        printf("H2 socketpair failed errno=%d\n", errno);
        sv[0] = sv[1] = -1;
    }
    if (sv[0] >= 0) {
        struct pollfd fd = { .fd = sv[0], .events = POLLIN, .revents = 0 };
        int ready = 0;
        for (int i = 0; i < 100000; i++) {
            fd.revents = 0;
            int pr = poll(&fd, 1, 0);
            if (pr > 0) { ready = i + 1; break; }
        }
        printf("H2 poll-empty-socketpair: phantom-ready=%s (%d)\n",
               ready ? "YES - BUG" : "no", ready);
        close(sv[0]); close(sv[1]);
    }

    /* H2b: same for a pipe read end. */
    int pfd[2];
    if (pipe(pfd) != 0) {
        printf("H2b pipe failed errno=%d\n", errno);
        pfd[0] = pfd[1] = -1;
    }
    if (pfd[0] >= 0) {
        struct pollfd fd = { .fd = pfd[0], .events = POLLIN, .revents = 0 };
        int ready = 0;
        for (int i = 0; i < 100000; i++) {
            fd.revents = 0;
            int pr = poll(&fd, 1, 0);
            if (pr > 0) { ready = i + 1; break; }
        }
        printf("H2b poll-empty-pipe: phantom-ready=%s (%d)\n",
               ready ? "YES - BUG" : "no", ready);
        close(pfd[0]); close(pfd[1]);
    }

    /* H2c: poll on a CONNECTED-but-idle unix socket the session bus way:
     * listener + accepted conn with no data. */
    {
        int lst = socket(AF_UNIX, SOCK_STREAM, 0);
        struct sockaddr_un sa;
        sa.sun_family = AF_UNIX;
        strcpy(sa.sun_path, "/tmp/spinprobe.sock");
        unlink("/tmp/spinprobe.sock");
        bind(lst, (struct sockaddr *)&sa, sizeof(sa));
        listen(lst, 1);
        int c = socket(AF_UNIX, SOCK_STREAM, 0);
        connect(c, (struct sockaddr *)&sa, sizeof(sa));
        int a = accept(lst, NULL, NULL);
        struct pollfd fd = { .fd = a, .events = POLLIN, .revents = 0 };
        int ready = 0;
        for (int i = 0; i < 100000; i++) {
            fd.revents = 0;
            int pr = poll(&fd, 1, 0);
            if (pr > 0) { ready = i + 1; break; }
        }
        printf("H2c poll-accepted-idle-conn: phantom-ready=%s (%d)\n",
               ready ? "YES - BUG" : "no", ready);
        close(a); close(c); close(lst);
        unlink("/tmp/spinprobe.sock");
    }

    /* H3: blocking waitpid sanity — sleeping child, reaped with status. */
    pid_t q = fork();
    if (q == 0) { usleep(50000); _exit(7); }
    int st2 = 0;
    pid_t r2 = waitpid(q, &st2, 0);
    printf("H3 blocking-wait: r=%d exit=%d (want r=%d exit=7)\n",
           (int)r2, r2 == q ? WEXITSTATUS(st2) : -1, (int)q);

    /* H4: SIGCHLD delivery count — glib's child watch needs SIGCHLD.
     * Count deliveries for 3 quick children. */
    {
        volatile int sigs = 0;
        (void)sigs;
        int total = 0;
        for (int k = 0; k < 3; k++) {
            pid_t w = fork();
            if (w == 0) _exit(1);
            int s3 = 0;
            pid_t r3 = waitpid(w, &s3, 0);
            if (r3 == w) total++;
        }
        printf("H4 reap-3-children: %d/3\n", total);
    }


    /* H2d: STICKY-POLLIN — after writing and fully consuming data, poll
     * must NOT report readiness. A sticky POLLIN spins any Xlib/GDK read
     * loop (poll->read EAGAIN->poll->...). Both socketpair and pipe. */
    {
        int ok = 1;
        if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0) {
            char buf[16];
            write(sv[1], "abcd", 4);
            ssize_t n = read(sv[0], buf, sizeof buf);
            struct pollfd fd = { .fd = sv[0], .events = POLLIN, .revents = 0 };
            int pr = poll(&fd, 1, 0);
            printf("H2d socketpair consumed %zd bytes then poll: r=%d revents=%x %s\n",
                   n, pr, fd.revents, pr == 0 ? "(good)" : "STICKY-BUG");
            if (pr != 0) ok = 0;
            close(sv[0]); close(sv[1]);
        }
        if (pipe(pfd) == 0) {
            char buf[16];
            write(pfd[1], "wxyz", 4);
            ssize_t n = read(pfd[0], buf, sizeof buf);
            struct pollfd fd = { .fd = pfd[0], .events = POLLIN, .revents = 0 };
            int pr = poll(&fd, 1, 0);
            printf("H2d pipe consumed %zd bytes then poll: r=%d revents=%x %s\n",
                   n, pr, fd.revents, pr == 0 ? "(good)" : "STICKY-BUG");
            if (pr != 0) ok = 0;
            close(pfd[0]); close(pfd[1]);
        }
        (void)ok;
        /* H2e: repeated write/read cycles then final poll (edge leftovers) */
        if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0) {
            char buf[4096];
            int sticky = 0;
            for (int i = 0; i < 50; i++) {
                write(sv[1], buf, sizeof buf);
                ssize_t total = 0;
                while (total < (ssize_t)sizeof buf) {
                    ssize_t n = read(sv[0], buf + total, sizeof(buf) - total);
                    if (n <= 0) break;
                    total += n;
                }
            }
            struct pollfd fd = { .fd = sv[0], .events = POLLIN, .revents = 0 };
            int pr = poll(&fd, 1, 0);
            if (pr != 0) sticky = 1;
            printf("H2e 50 write/consume cycles then poll: r=%d %s\n",
                   pr, sticky ? "STICKY-BUG" : "(good)");
            close(sv[0]); close(sv[1]);
        }
    }

    /* H5: SIGCHLD redelivery flood — one child, reaped, then watch for
     * spurious additional SIGCHLDs for 300ms. */
    {
        volatile int sig_count = 0;
        (void)sig_count;
        sigset_t set;
        sigemptyset(&set);
        sigaddset(&set, SIGCHLD);
        sigprocmask(SIG_BLOCK, &set, NULL);
        pid_t p = fork();
        if (p == 0) _exit(5);
        usleep(120000);              /* child exits, SIGCHLD pending (blocked) */
        siginfo_t si;
        memset(&si, 0, sizeof si);
        int got = sigwaitinfo(&set, &si);
        printf("H5 sigwaitinfo: got=%d (%s) code=%d pid=%d\n", got,
               got == SIGCHLD ? "SIGCHLD" : "other", si.si_code, si.si_pid);
        /* now check for REDLIVERY: pending must be empty */
        sigset_t pend;
        sigemptyset(&pend);
        sigpending(&pend);
        int still = sigismember(&pend, SIGCHLD);
        printf("H5 after-consume pending SIGCHLD: %s\n",
               still ? "STILL-PENDING-BUG" : "clear(good)");
        int extra_waits = 0;
        for (int i = 0; i < 5; i++) {
            struct timespec ts = { 0, 50000000 }; /* 50ms */
            int g2 = sigtimedwait(&set, &si, &ts);
            if (g2 == SIGCHLD) extra_waits++;
        }
        printf("H5 redeliveries in 250ms: %d %s\n", extra_waits,
               extra_waits ? "FLOOD-BUG" : "(good)");
        sigprocmask(SIG_UNBLOCK, &set, NULL);
    }

    printf("PROBE DONE\n");
    return 0;
}
