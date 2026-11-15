/*
 * et_fd_probe — the EPOLLET lost-edge pattern across fd types.
 *
 * Extension of tfd_et_probe.c (the timerfd case, fixed 2026-11-06 by
 * d3a94b4d). The same bug class exists wherever a producer raises fd
 * readiness OUTSIDE the watcher's epoll_wait and the fd is NEVER
 * drained (the FFRT / GLib / OH service-loop shape):
 *
 *   epoll_ctl(ADD, fd, EPOLLIN|EPOLLET)
 *   loop { producer_write();          — edge lands between waits
 *          epoll_wait(500ms); }       — never reads the fd
 *
 * A mask-unchanged==no-new-edge ET heuristic suppresses the report
 * forever once one edge was reported and the fd stayed readable —
 * unless the producer's wake path also notifies the epoll registry
 * (the reference's ep_poll_callback rides the file wait queue for ALL
 * fd types). timerfd got that notify; every other fd type must be
 * probed before patching (mission rule: test BEFORE patching).
 *
 * Per-type verdict lines (Linux reference: miss=0 for ALL rows):
 *   ETPROBE-DONE type=timerfd  rounds=N reported=R miss=M
 *   ETPROBE-DONE type=eventfd  ...
 *   ETPROBE-DONE type=pipe     ...
 *   ETPROBE-DONE type=unixsock ...   (control: unix.rs already notifies)
 *
 * Each round writes ONCE between waits and never drains, so every
 * round after the first is the discriminating never-idle edge.
 */
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <sys/timerfd.h>
#include <time.h>

#define ROUNDS 300

static uint64_t now_ns(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

/* Shared round loop: `produce` fires one edge between waits; the fd is
 * never read. Returns 0 on full pass, nonzero on miss/error. */
static int run_rounds(int ep, const char *type, void (*produce)(void), int rounds)
{
    int reported = 0, miss = 0;
    struct epoll_event out;

    for (int i = 0; i < rounds; i++) {
        produce();
        /* Small gap on every 5th round: the write lands while nobody
         * waits (matches tfd_et_probe's gap discipline; the plain
         * rounds additionally cover write-just-before-wait). */
        if (i % 5 == 0)
            usleep(1000);
        int nfds = epoll_wait(ep, &out, 1, 500);
        if (nfds == 1 && (out.events & EPOLLIN)) {
            reported++;
        } else if (nfds == 0) {
            miss++;
            if (miss <= 5)
                printf("ETPROBE-MISS type=%s round=%d\n", type, i);
        } else {
            printf("ETPROBE-ERR type=%s round=%d nfds=%d errno=%d\n",
                   type, i, nfds, errno);
            return 1;
        }
    }
    printf("ETPROBE-DONE type=%s rounds=%d reported=%d miss=%d\n",
           type, rounds, reported, miss);
    return miss ? 2 : 0;
}

/* --- producers --- */
static int g_tfd = -1, g_efd = -1, g_pw = -1, g_us_peer = -1;

static void produce_timerfd(void)
{
    /* One-shot 1ms relative arm; the expiry is the edge. */
    struct itimerspec its;
    memset(&its, 0, sizeof(its));
    its.it_value.tv_nsec = 1 * 1000 * 1000;
    timerfd_settime(g_tfd, 0, &its, NULL);
}

static void produce_eventfd(void)
{
    uint64_t one = 1;
    if (write(g_efd, &one, sizeof(one)) != 8)
        printf("ETPROBE-ERR eventfd write errno=%d\n", errno);
}

static void produce_pipe(void)
{
    char c = 'x';
    if (write(g_pw, &c, 1) != 1)
        printf("ETPROBE-ERR pipe write errno=%d\n", errno);
}

static void produce_unix(void)
{
    char c = 'y';
    if (write(g_us_peer, &c, 1) != 1)
        printf("ETPROBE-ERR unix write errno=%d\n", errno);
}

/* --- TCP over loopback: EPOLLET on the accepted conn, peer writes --- */
static int g_tcp_peer = -1;

static void produce_tcp(void)
{
    char c = 'z';
    if (write(g_tcp_peer, &c, 1) != 1)
        printf("ETPROBE-ERR tcp write errno=%d\n", errno);
}

static int setup_tcp(void)
{
    int lst = socket(AF_INET, SOCK_STREAM, 0);
    int cli = socket(AF_INET, SOCK_STREAM, 0);
    if (lst < 0 || cli < 0)
        return -1;
    struct sockaddr_in a;
    memset(&a, 0, sizeof(a));
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(0x7f000001U); /* 127.0.0.1 */
    a.sin_port = htons(41234);
    if (bind(lst, (struct sockaddr *)&a, sizeof(a)) != 0 || listen(lst, 1) != 0)
        return -1;
    if (connect(cli, (struct sockaddr *)&a, sizeof(a)) != 0)
        return -1;
    int conn = accept(lst, NULL, NULL);
    if (conn < 0)
        return -1;
    /* One byte per round is never drained: the same never-read shape
     * as FFRT's wake patterns. */
    g_tcp_peer = cli;
    return conn;
}

/* --- UDP over loopback: EPOLLET on the bound receiver, peer sends --- */
static int g_udp_peer = -1;

static void produce_udp(void)
{
    char c = 'w';
    static struct sockaddr_in dst;
    memset(&dst, 0, sizeof(dst));
    dst.sin_family = AF_INET;
    dst.sin_addr.s_addr = htonl(0x7f000001U);
    dst.sin_port = htons(41235);
    if (sendto(g_udp_peer, &c, 1, 0, (struct sockaddr *)&dst, sizeof(dst)) != 1)
        printf("ETPROBE-ERR udp send errno=%d\n", errno);
}

static int setup_udp(void)
{
    int rx = socket(AF_INET, SOCK_DGRAM, 0);
    int tx = socket(AF_INET, SOCK_DGRAM, 0);
    if (rx < 0 || tx < 0)
        return -1;
    struct sockaddr_in a;
    memset(&a, 0, sizeof(a));
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(0x7f000001U);
    a.sin_port = htons(41235);
    if (bind(rx, (struct sockaddr *)&a, sizeof(a)) != 0)
        return -1;
    g_udp_peer = tx;
    return rx;
}

/* --- type setup: return the fd to watch (producer state in globals) --- */
static int setup_timerfd(void)
{
    g_tfd = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK | TFD_CLOEXEC);
    return g_tfd;
}

static int setup_eventfd(void)
{
    g_efd = eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC);
    return g_efd;
}

static int g_pr = -1;

static int setup_pipe(void)
{
    int pfd[2];
    if (pipe(pfd) != 0)
        return -1;
    g_pr = pfd[0];
    g_pw = pfd[1];
    return g_pr;
}

static int g_us = -1;

static int setup_unix(void)
{
    int sv[2];
    /* SOCK_NONBLOCK: the never-drained peer accumulates per-write skb
     * overhead; a blocking write would wedge the probe at the buffer
     * edge (~round 277 on Linux — measured), killing the boot (the
     * probe runs as PID 1). Nonblocking keeps the probe safe on any
     * kernel; an EAGAIN means the buffer filled and the row is void. */
    if (socketpair(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0, sv) != 0)
        return -1;
    g_us = sv[0];
    g_us_peer = sv[1];
    return g_us;
}

static int run_type(const char *type, int (*setup)(void), void (*produce)(void), int rounds)
{
    int fd = setup();
    if (fd < 0) {
        printf("ETPROBE-FAIL setup type=%s errno=%d\n", type, errno);
        return 1;
    }
    int ep = epoll_create1(EPOLL_CLOEXEC);
    if (ep < 0) {
        printf("ETPROBE-FAIL epoll type=%s errno=%d\n", type, errno);
        return 1;
    }
    struct epoll_event ev;
    memset(&ev, 0, sizeof(ev));
    ev.events = EPOLLIN | EPOLLET;
    ev.data.fd = fd;
    if (epoll_ctl(ep, EPOLL_CTL_ADD, fd, &ev) != 0) {
        printf("ETPROBE-FAIL ctl type=%s errno=%d\n", type, errno);
        return 1;
    }
    int rc = run_rounds(ep, type, produce, rounds);
    close(ep);
    close(fd);
    /* Peer/other ends closed by the process exit; keep it simple. */
    return rc;
}

int main(void)
{
    int rc = 0, r;
    uint64_t t0 = now_ns();

    setvbuf(stdout, NULL, _IONBF, 0);

    r = run_type("timerfd", setup_timerfd, produce_timerfd, ROUNDS);
    printf("ETPROBE-RC type=timerfd rc=%d\n", r);
    rc |= (r == 1);

    r = run_type("eventfd", setup_eventfd, produce_eventfd, ROUNDS);
    printf("ETPROBE-RC type=eventfd rc=%d\n", r);
    rc |= (r == 1);

    r = run_type("pipe", setup_pipe, produce_pipe, ROUNDS);
    printf("ETPROBE-RC type=pipe rc=%d\n", r);
    rc |= (r == 1);

    /* unix control: 60 rounds — the never-drained peer's skb overhead
     * would outgrow the receive buffer near ~277 rounds (measured on
     * the reference; EAGAIN voids the row). 60 rounds is far inside
     * the safe margin on both kernels and still exercises the notify
     * path 60 times. */
    r = run_type("unixsock", setup_unix, produce_unix, 60);
    printf("ETPROBE-RC type=unixsock rc=%d\n", r);
    rc |= (r == 1);

    r = run_type("tcp", setup_tcp, produce_tcp, 60);
    printf("ETPROBE-RC type=tcp rc=%d\n", r);
    rc |= (r == 1);

    r = run_type("udp", setup_udp, produce_udp, 60);
    printf("ETPROBE-RC type=udp rc=%d\n", r);
    rc |= (r == 1);

    printf("ETPROBE-ALL-DONE secs=%lu rc=%d\n",
           (unsigned long)((now_ns() - t0) / 1000000000ull), rc);
    return rc;
}
