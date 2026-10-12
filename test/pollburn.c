// pollburn - idle-wait poll/epoll CPU burn repro for Rux.
//
// On Linux, blocking on an idle fd costs ~0 CPU. This program blocks on a
// quiet AF_UNIX socketpair for N seconds and then reports:
//   - how many times the wait syscall returned (spurious wakeups)
//   - utime+stime consumed by the process (/proc/self/stat, CLK_TCK ticks)
//
// usage: pollburn <poll|ppoll|epoll|select> <seconds> [quiet]
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <poll.h>
#include <sys/epoll.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <time.h>
#include <signal.h>
#include <sys/wait.h>

static int self_cpu_ticks(void)
{
    FILE *f = fopen("/proc/self/stat", "r");
    if (!f) return -1;
    char buf[8192];
    size_t n = fread(buf, 1, sizeof(buf) - 1, f);
    fclose(f);
    buf[n] = 0;
    // fields 14 (utime) and 15 (stime); comm may contain spaces
    char *p = strrchr(buf, ')');
    if (!p) return -1;
    p += 2; // skip ") S "
    long ut = 0, st = 0;
    int fno = 3; // next field after state is #4
    char *tok = strtok(p, " ");
    while (tok) {
        if (fno == 14) ut = atol(tok);
        else if (fno == 15) { st = atol(tok); break; }
        tok = strtok(NULL, " ");
        fno++;
    }
    return (int)(ut + st);
}

static double now_s(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec / 1e9;
}

int main(int argc, char **argv)
{
    if (argc < 3) {
        fprintf(stderr, "usage: pollburn <poll|ppoll|epoll|select> <seconds>\n");
        return 2;
    }
    const char *mode = argv[1];
    int seconds = atoi(argv[2]);

    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        perror("socketpair");
        return 1;
    }
    // Peer sv[1] stays open but silent: fd sv[0] is idle, not HUP.

    int efd = -1;
    if (!strcmp(mode, "epoll")) {
        efd = epoll_create1(0);
        if (efd < 0) { perror("epoll_create1"); return 1; }
        struct epoll_event ev = { .events = EPOLLIN, .data.u64 = 42 };
        if (epoll_ctl(efd, EPOLL_CTL_ADD, sv[0], &ev) < 0) {
            perror("epoll_ctl"); return 1;
        }
    }

    int t0 = self_cpu_ticks();
    double w0 = now_s();
    long returns = 0, nonzero = 0;

    if (!strcmp(mode, "busy")) {
        // control: spin in userspace; validates tick accounting
        volatile long x = 0;
        while (now_s() - w0 < seconds) x++;
        (void)x;
    }

    if (!strcmp(mode, "wake")) {
        // Producer-wake latency: child sleeps 1s then writes sv[1]; parent
        // blocks in poll(sv[0], POLLIN, 10s). Must return ~1s after the
        // write (event-driven wake), not at the 10s timeout.
        pid_t c = fork();
        if (c == 0) {
            sleep(1);
            write(sv[1], "x", 1);
            _exit(0);
        }
        double t_start = now_s();
        struct pollfd p = { .fd = sv[0], .events = POLLIN, .revents = 0 };
        int r = poll(&p, 1, 10000);
        double dt = now_s() - t_start;
        int status;
        waitpid(c, &status, 0);
        printf("POLLBURN mode=wake poll_ret=%d revents=%04x latency=%.3fs "
               "(expect ~1s; 10s = missed producer wake)\n",
               r, p.revents, dt);
        return 0;
    }

    while (now_s() - w0 < seconds) {
        int r;
        if (!strcmp(mode, "poll") || !strcmp(mode, "ppoll")) {
            struct pollfd p = { .fd = sv[0], .events = POLLIN, .revents = 0 };
            if (!strcmp(mode, "poll"))
                r = poll(&p, 1, 1000);
            else {
                struct timespec ts = { .tv_sec = 1, .tv_nsec = 0 };
                sigset_t empty; sigemptyset(&empty);
                r = ppoll(&p, 1, &ts, &empty);
            }
            if (r != 0) returns++, nonzero += (r > 0);
        } else if (!strcmp(mode, "epoll")) {
            struct epoll_event out[4];
            r = epoll_wait(efd, out, 4, 1000);
            if (r != 0) returns++, nonzero += (r > 0);
        } else {
            struct timeval tv = { .tv_sec = 1, .tv_usec = 0 };
            fd_set rfds;
            FD_ZERO(&rfds);
            FD_SET(sv[0], &rfds);
            r = select(sv[0] + 1, &rfds, NULL, NULL, &tv);
            if (r != 0) returns++, nonzero += (r > 0);
        }
        (void)r;
    }

    int t1 = self_cpu_ticks();
    double wall = now_s() - w0;
    printf("POLLBURN mode=%s wall=%.1fs cpu_ticks=%d (%.1f%% of one core) "
           "wait_returns=%ld ready_gt0=%ld\n",
           mode, wall, t1 - t0, 100.0 * (t1 - t0) / 100.0 / wall,
           returns, nonzero);
    return 0;
}
