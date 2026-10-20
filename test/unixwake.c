/*
 * unixwake.c — reproduce the appspawn-server wedge shape in isolation.
 *
 * b10/b11: AMS's appspawn client connect()s and writes 1056 bytes to
 * /dev/unix/socket/AppSpawn; the appspawn server (LE epoll loop) never
 * accepts/reads — clients time out after 2 s (EAGAIN) and retry forever.
 * This probe builds the same shape: a listening AF_UNIX stream socket
 * registered in epoll, then a child process connects and writes. If the
 * parent's epoll_wait never reports the listener, the kernel loses the
 * connect/readiness wake (lost-wake family — same suspect as the
 * coordinator's 41.5 s vsync dead stop).
 *
 * usage: unixwake [path]   (default /tmp/unixwake.sock)
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

static double now_ms(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000.0 + ts.tv_nsec / 1e6;
}

int main(int argc, char **argv)
{
    const char *path = (argc > 1) ? argv[1] : "/tmp/unixwake.sock";
    unlink(path);

    int lfd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (lfd < 0) { perror("socket"); return 1; }
    struct sockaddr_un sa = {0};
    sa.sun_family = AF_UNIX;
    strncpy(sa.sun_path, path, sizeof(sa.sun_path) - 1);
    if (bind(lfd, (struct sockaddr *)&sa, sizeof(sa)) < 0) { perror("bind"); return 1; }
    if (listen(lfd, 4) < 0) { perror("listen"); return 1; }

    int ep = epoll_create1(0);
    if (ep < 0) { perror("epoll_create1"); return 1; }
    struct epoll_event ev = { .events = EPOLLIN, .data.fd = lfd };
    if (epoll_ctl(ep, EPOLL_CTL_ADD, lfd, &ev) < 0) { perror("epoll_ctl"); return 1; }

    fprintf(stderr, "parent: waiting on epoll (listener %d)\n", lfd);
    /* THREE sequential connections on one listener: the appspawn shape
     * is connect #1 accepted early (cold-start child), then later
     * connects never wake the loop — i.e. a readiness re-arm bug after
     * a prior accept. The original probe accepted once and exited,
     * which cannot see that. */
    for (int round = 0; round < 3; round++) {
        pid_t child = fork();
        if (child == 0) {
            usleep(200 * 1000);
            int cfd = socket(AF_UNIX, SOCK_STREAM, 0);
            if (cfd < 0) { perror("child socket"); _exit(1); }
            if (connect(cfd, (struct sockaddr *)&sa, sizeof(sa)) < 0) {
                perror("child connect"); _exit(2);
            }
            char buf[1056];
            memset(buf, 'A', sizeof(buf));
            ssize_t w = write(cfd, buf, sizeof(buf));
            fprintf(stderr, "child%d: connect ok, wrote %zd\n", round, w);
            usleep(1500 * 1000);
            close(cfd);
            _exit(0);
        }
        double t0 = now_ms();
        struct epoll_event out;
        int n = epoll_wait(ep, &out, 1, 4000);
        double dt = now_ms() - t0;
        if (n < 0) {
            perror("epoll_wait");
        } else if (n == 0) {
            fprintf(stderr, "parent: round %d TIMEOUT after %.1f ms — listener never ready (LOST WAKE)\n", round, dt);
        } else {
            fprintf(stderr, "parent: round %d epoll_wait %d after %.1f ms fd=%d ev=%#x — accepting\n",
                    round, n, dt, out.data.fd, out.events);
            int cfd = accept(lfd, NULL, NULL);
            if (cfd < 0) {
                perror("accept");
            } else {
                char buf[1100];
                ssize_t r = read(cfd, buf, sizeof(buf));
                fprintf(stderr, "parent: round %d accepted %d, read %zd\n", round, cfd, r);
                close(cfd);
            }
        }
        int st; waitpid(child, &st, 0);
    }
    fprintf(stderr, "parent: done\n");

    /* Mode 2: socketpair peer death. The appspawn server's cold-start
     * child (forked early, exits without exec on b10/b11) shares a
     * parentToChild socketpair with the server. If a blocking read on
     * the surviving end never sees EOF after the peer exits, the server
     * main thread hangs forever — the observed wedge shape. */
    {
        int sv[2];
        if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) { perror("socketpair"); return 0; }
        pid_t c2 = fork();
        if (c2 == 0) {
            close(sv[0]);
            _exit(0); /* peer dies without writing */
        }
        close(sv[1]);
        char b2[64];
        fprintf(stderr, "sp-test: blocking read after peer death...\n");
        double s0 = now_ms();
        ssize_t r2 = read(sv[0], b2, sizeof(b2));
        double sd = now_ms() - s0;
        if (r2 < 0) {
            fprintf(stderr, "sp-test: read err %s after %.1f ms\n", strerror(errno), sd);
        } else if (r2 == 0) {
            fprintf(stderr, "sp-test: EOF after %.1f ms (correct)\n", sd);
        } else {
            fprintf(stderr, "sp-test: got %zd bytes after %.1f ms (unexpected)\n", r2, sd);
        }
        /* also: does the surviving end report EPOLLIN/HUP? */
        int ep2 = epoll_create1(0);
        struct epoll_event ev2 = { .events = EPOLLIN, .data.fd = sv[0] };
        epoll_ctl(ep2, EPOLL_CTL_ADD, sv[0], &ev2);
        struct epoll_event out2;
        int n2 = epoll_wait(ep2, &out2, 1, 1000);
        fprintf(stderr, "sp-test: epoll on dead-peer fd: n=%d ev=%#x\n", n2,
                n2 > 0 ? out2.events : 0);
        int st2; waitpid(c2, &st2, 0);
        close(sv[0]);
    }
    return 0;
}
