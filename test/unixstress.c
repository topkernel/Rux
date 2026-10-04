/* unixstress.c — AF_UNIX SOCK_STREAM multi-client sustained-load probe.
 *
 * Reproduces the GNOME session-bus failure shape (GF2): a healthy server
 * (dbus-daemon analogue) + many persistent clients; after sustained load
 * some client sees a spurious EOF / EPIPE / permanent wedge while the
 * server keeps running.
 *
 * Topology (fork only, no threads — matches GNOME's multi-process bus):
 *   supervisor (main)
 *     +-- server   : poll()-loop echo daemon, non-blocking fds, dbus shape
 *     +-- N clients: blocking write request / read reply, seq+crc verified
 *     +-- chaos    : churn client, connects/aborts every few seconds
 *
 * Wire frame (little-endian):
 *   u32 magic 'R','U','X','1' | u32 cid | u32 seq | u32 len | u32 crc32
 *   + len bytes payload (deterministic pseudo-random from seq)
 *
 * Detectors:
 *   - EOF_UNEXPECTED  : read()==0 while connection should be alive
 *                       (the kernel turning live streams into EOF)
 *   - EPIPE           : write() EPIPE while the peer never closed
 *   - WEDGE           : poll() 30s timeout on a healthy connection
 *   - DATA            : crc/seq mismatch
 *
 * Phase 0 quick semantics:
 *   T1 zero-length write must be a no-op (no EOF for the peer)
 *   T2 backpressure: 600KB write with a stalled reader must BLOCK
 *      (Linux semantics), then complete — never EPIPE.
 *
 * Exit codes: 0 PASS, 1 setup failure, 2 phase-0 failure, 3..6 as above
 * (per client). Supervisor prints VERDICT: PASS/FAIL.
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <time.h>
#include <poll.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <sys/mount.h>
#include <sys/epoll.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <math.h>

#define HDR_LEN     20
#define MAGIC0      0x31585552 /* "RUX1" LE */
#define MAX_CLIENTS 64
#define POLL_TO_MS  30000     /* 30s watchdog for one poll round */
#define STALL_EVERY 30        /* server stalls reads every N sec */
#define STALL_FOR   5         /* ... for N sec (backpressure waves) */

static int duration = 300;
static int nclients = 12;
static int use_abstract = 0;
static const char *SOCK_PATH = "/tmp/unixstress.sock";
static const char *ABS_NAME  = "unixstress";

static double mono(void)
{
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) == 0)
        return ts.tv_sec + ts.tv_nsec / 1e9;
    return (double)time(NULL);
}

static void tsleep(double s)
{
    struct timespec ts = { (time_t)s, (long)((s - (time_t)s) * 1e9) };
    nanosleep(&ts, NULL);
}

/* deterministic payload from (seq, byte index) */
static void fill_payload(unsigned char *p, unsigned seq, unsigned len)
{
    unsigned x = seq * 2654435761u + 12345u;
    for (unsigned i = 0; i < len; i++) {
        x = x * 16677719u + 2246822519u; /* FNV-ish */
        p[i] = (unsigned char)(x >> 24);
    }
}

static unsigned crc32_buf(const unsigned char *p, unsigned len)
{
    unsigned h = 0x811c9dc5u;
    for (unsigned i = 0; i < len; i++) {
        h ^= p[i];
        h *= 0x01000193u;
    }
    return h;
}

static void put_u32(unsigned char *p, unsigned v)
{
    p[0] = v; p[1] = v >> 8; p[2] = v >> 16; p[3] = v >> 24;
}
static unsigned get_u32(const unsigned char *p)
{
    return p[0] | (p[1] << 8) | (p[2] << 16) | ((unsigned)p[3] << 24);
}

static void mk_addr(struct sockaddr_un *a, socklen_t *alen)
{
    memset(a, 0, sizeof *a);
    a->sun_family = AF_UNIX;
    if (use_abstract) {
        a->sun_path[0] = '\0';
        strncpy(a->sun_path + 1, ABS_NAME, sizeof a->sun_path - 2);
        *alen = 2 + 1 + strlen(ABS_NAME);
    } else {
        strncpy(a->sun_path, SOCK_PATH, sizeof a->sun_path - 1);
        *alen = 2 + strlen(SOCK_PATH) + 1;
    }
}

/* full write (loop over partials); returns 0 ok, -1 errno */
static int write_all(int fd, const void *buf, size_t n, int *out_errno)
{
    const char *p = buf;
    while (n > 0) {
        ssize_t w = write(fd, p, n);
        if (w < 0) {
            if (errno == EINTR) continue;
            *out_errno = errno;
            return -1;
        }
        if (w == 0) { *out_errno = 0; return -1; }
        p += w; n -= (size_t)w;
    }
    return 0;
}

/* read exactly n bytes; 0 on EOF-before-any, 1 ok, -1 errno */
static int read_exact(int fd, void *buf, size_t n, int *out_errno)
{
    char *p = buf;
    size_t got = 0;
    while (got < n) {
        struct pollfd pf = { .fd = fd, .events = POLLIN };
        int pr = poll(&pf, 1, POLL_TO_MS);
        if (pr < 0) {
            if (errno == EINTR) continue;
            *out_errno = errno;
            return -1;
        }
        if (pr == 0) { *out_errno = ETIMEDOUT; return -1; }
        ssize_t r = read(fd, p + got, n - got);
        if (r < 0) {
            if (errno == EINTR) continue;
            *out_errno = errno;
            return -1;
        }
        if (r == 0) {
            if (got == 0) { *out_errno = 0; return 0; }
            *out_errno = EIO; /* EOF mid-message = truncated stream */
            return -1;
        }
        got += (size_t)r;
    }
    return 1;
}

static void dump_netunix(void)
{
    FILE *f = fopen("/proc/net/unix", "r");
    if (!f) { printf("    [/proc/net/unix unavailable]\n"); return; }
    char line[256];
    int n = 0;
    printf("    --- /proc/net/unix ---\n");
    while (fgets(line, sizeof line, f) && n < 24) {
        fputs("    ", stdout); fputs(line, stdout);
        n++;
    }
    fclose(f);
}

/* ============================================================
 * Phase 0 quick semantics
 * ==========================================================*/
static int phase0(void)
{
    int fails = 0;
    int ls = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un a; socklen_t alen;
    mk_addr(&a, &alen);
    unlink(SOCK_PATH);
    if (bind(ls, (struct sockaddr *)&a, alen) < 0 || listen(ls, 4) < 0) {
        printf("P0 setup FAIL %s\n", strerror(errno));
        return 1;
    }

    /* T1: zero-length write is a no-op — must NOT read as EOF */
    {
        int c = socket(AF_UNIX, SOCK_STREAM, 0);
        if (connect(c, (struct sockaddr *)&a, alen) < 0) {
            printf("P0.T1 connect FAIL %s\n", strerror(errno));
            close(c); close(ls);
            return 1;
        }
        int ac = accept(ls, NULL, NULL);
        if (ac < 0) { printf("P0.T1 accept FAIL\n"); close(c); close(ls); return 1; }
        ssize_t w = write(c, "", 0);
        /* give the kernel a moment, then server sends real data */
        tsleep(0.2);
        int e;
        write_all(ac, "REAL", 4, &e);
        char b[8] = { 0 };
        int r = read_exact(c, b, 4, &e);
        int ok = (r == 1) && w == 0 && memcmp(b, "REAL", 4) == 0;
        printf("P0.T1 zero-write-no-eof %s (w=%zd r=%d b=%.4s errno=%d)\n",
               ok ? "PASS" : "FAIL", w, r, b, e);
        if (!ok) { fails++; dump_netunix(); }
        close(c); close(ac);
    }

    /* T2: backpressure — 600KB into a stalled reader must BLOCK, not EPIPE */
    {
        int sv[2];
        if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
            printf("P0.T2 socketpair FAIL %s\n", strerror(errno));
            close(ls);
            return 1;
        }
        size_t big = 600 * 1024;
        unsigned char *buf = malloc(big);
        memset(buf, 0x5a, big);
        /* signal path: child writes after 1.5s; parent sleeps 4s then reads */
        pid_t ch = fork();
        if (ch == 0) {
            close(sv[0]);
            tsleep(1.5);
            double t0 = mono();
            int e;
            int rc = write_all(sv[1], buf, big, &e);
            double dt = mono() - t0;
            if (rc < 0)
                printf("P0.T2 child write FAIL errno=%d (%s) after %.2fs\n",
                       e, e == EPIPE ? "EPIPE!" : strerror(e), dt);
            else
                printf("P0.T2 child write OK (%.2fs blocked-then-done)\n", dt);
            fflush(stdout);
            _exit(rc == 0 ? 0 : 7);
        }
        close(sv[1]);
        tsleep(4.0); /* let the child fill sndbuf and block */
        size_t drained = 0;
        for (;;) {
            struct pollfd pf = { .fd = sv[0], .events = POLLIN };
            if (poll(&pf, 1, 5000) != 1) break;
            ssize_t r = read(sv[0], buf, big);
            if (r <= 0) break;
            drained += (size_t)r;
        }
        int st; waitpid(ch, &st, 0);
        int child_ok = WIFEXITED(st) && WEXITSTATUS(st) == 0;
        int ok = child_ok && drained == big;
        printf("P0.T2 backpressure-blocks %s (drained=%zu/%zu child=%d)\n",
               ok ? "PASS" : "FAIL", drained, big,
               WIFEXITED(st) ? WEXITSTATUS(st) : -9);
        if (!ok) fails++;
        close(sv[0]);
        free(buf);
    }

    /* T3: EPOLLET edge must survive maxevents truncation. K fds all ready,
     * epoll_wait(maxevents=2) repeatedly must eventually report ALL K —
     * Linux keeps undelivered ready events; consuming an edge without
     * delivering it wedges the fd forever (the Xorg client-socket shape).
     */
    {
        enum { K = 6 };
        int ep = epoll_create1(0);
        int sv[K][2];
        int registered = 0;
        for (int i = 0; i < K; i++) {
            if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv[i]) == 0) {
                struct epoll_event ev = { .events = EPOLLIN | EPOLLET,
                                          .data.u32 = (unsigned)i };
                if (epoll_ctl(ep, EPOLL_CTL_ADD, sv[i][1], &ev) == 0)
                    registered++;
            }
        }
        /* make every registered fd ready at once */
        for (int i = 0; i < K; i++)
            write(sv[i][0], "x", 1);
        unsigned seen_mask = 0;
        struct epoll_event out[2];
        for (int round = 0; round < 200 && seen_mask != (1u << K) - 1; round++) {
            int n = epoll_wait(ep, out, 2, 50);
            if (n <= 0) break;
            for (int j = 0; j < n; j++)
                seen_mask |= 1u << out[j].data.u32;
            /* do NOT read anything: the edges must still be delivered */
        }
        int ok = registered == K && seen_mask == (1u << K) - 1;
        printf("P0.T3 et-truncation %s (registered=%d seen=%x want=%x)\n",
               ok ? "PASS" : "FAIL", registered, seen_mask, (1u << K) - 1);
        if (!ok) fails++;
        close(ep);
        for (int i = 0; i < K; i++) { close(sv[i][0]); close(sv[i][1]); }
    }

    close(ls);
    unlink(SOCK_PATH);
    return fails;
}

/* ============================================================
 * Server (dbus-daemon analogue)
 * ==========================================================*/
#define SRV_MAXFDS 20
/* ET driver maxevents: deliberately SMALLER than the client count so
 * ready bursts exceed it (the Xorg dispatch shape). */
#define ET_MAXEV   8
/* conn buffers sized so one max frame (64KB+hdr) fits: in holds what the
 * kernel handed us, out holds echo backlog while the peer drains. Heap
 * arena, NOT static: a 36MB bss segment breaks the kernel ELF loader. */
struct conn {
    int fd;
    unsigned char in[80 * 1024];
    unsigned in_len;
    unsigned char out[96 * 1024];
    unsigned out_len, out_off;
    unsigned frames;
};

static void srv_send_frame(struct conn *cn, const unsigned char *hdr,
                           const unsigned char *payload, unsigned len)
{
    if (cn->out_len + HDR_LEN + len <= sizeof cn->out) {
        memcpy(cn->out + cn->out_len, hdr, HDR_LEN);
        if (len) memcpy(cn->out + cn->out_len + HDR_LEN, payload, len);
        cn->out_len += HDR_LEN + len;
    }
    /* caller checks room first — a full out stops reads (flow control) */
}

/* shared server state (both poll and epoll/ET drivers) */
static struct conn *conns;
static int nconn;
static unsigned total_frames, accepts;
static int use_et; /* server drives with epoll+EPOLLET (Xorg shape) */

static void srv_compact(void)
{
    int w = 0;
    for (int i = 0; i < nconn; i++)
        if (conns[i].fd >= 0) conns[w++] = conns[i];
    nconn = w;
}

static struct conn *srv_find(int fd)
{
    for (int i = 0; i < nconn; i++)
        if (conns[i].fd == fd) return &conns[i];
    return NULL;
}

/* one accept sweep on the (nonblocking) listener */
static void srv_accept(int ls, int ep)
{
    for (;;) {
        int c = accept4(ls, NULL, NULL, SOCK_NONBLOCK);
        if (c < 0) break;
        if (nconn >= SRV_MAXFDS) {
            close(c);
            continue;
        }
        memset(&conns[nconn], 0, sizeof conns[nconn]);
        conns[nconn].fd = c;
        nconn++;
        accepts++;
        if (ep >= 0) {
            struct epoll_event ev = { .events = EPOLLIN | EPOLLET,
                                      .data.fd = c };
            epoll_ctl(ep, EPOLL_CTL_ADD, c, &ev);
        }
    }
}

/* read-ready handler: drain to EAGAIN, parse frames, queue echoes.
 * returns 0 ok, -1 conn dead (closed) */
static int srv_readable(struct conn *cn)
{
    int first = 1; /* first read after a readiness report */
    for (;;) {
        if (cn->in_len == sizeof cn->in) break; /* full: parse below */
        size_t want = sizeof cn->in - cn->in_len;
        ssize_t r = read(cn->fd, cn->in + cn->in_len, want);
        if (r < 0) {
            if (errno == EAGAIN || errno == EWOULDBLOCK) {
                if (first)
                    printf("SRV SPIN-READ fd=%d — ready then EAGAIN "
                           "(level bug)\n", cn->fd);
                break;
            }
            if (errno == EINTR) continue;
            printf("SRV conn fd=%d read FAIL %s\n", cn->fd, strerror(errno));
            return -1; /* treat as EOF */
        }
        if (r == 0) return -1; /* peer closed */
        first = 0;
        cn->in_len += (unsigned)r;
        if ((size_t)r < want) break; /* got what was available */
    }
    /* parse complete frames; stop when the echo buffer is too full to
     * mirror the next frame (flow control) */
    unsigned off = 0;
    while (cn->in_len - off >= HDR_LEN) {
        unsigned magic = get_u32(cn->in + off);
        unsigned len = get_u32(cn->in + off + 12);
        if (magic != MAGIC0) {
            printf("SRV FRAME DESYNC fd=%d magic=%#x\n", cn->fd, magic);
            return -1;
        }
        if (len > 96 * 1024) {
            printf("SRV FRAME BOGUS len=%u\n", len);
            len = 96 * 1024;
        }
        if (cn->in_len - off < HDR_LEN + len) break;
        if (cn->out_len + HDR_LEN + len > sizeof cn->out) break;
        srv_send_frame(cn, cn->in + off, cn->in + off + HDR_LEN, len);
        cn->frames++; total_frames++;
        off += HDR_LEN + len;
    }
    if (off) {
        memmove(cn->in, cn->in + off, cn->in_len - off);
        cn->in_len -= off;
    }
    return 0;
}

/* write-ready handler: flush queued echoes.
 * returns 0 ok, -1 conn dead */
static int srv_writable(struct conn *cn, int poll_reported_writable)
{
    if (cn->out_len > cn->out_off) {
        ssize_t w = write(cn->fd, cn->out + cn->out_off,
                          cn->out_len - cn->out_off);
        if (w > 0) {
            cn->out_off += (unsigned)w;
            if (cn->out_off == cn->out_len)
                cn->out_off = cn->out_len = 0;
        } else if (w < 0 && errno != EAGAIN && errno != EWOULDBLOCK &&
                   errno != EINTR) {
            printf("SRV conn fd=%d write FAIL %s (out=%u)\n", cn->fd,
                   strerror(errno), cn->out_len - cn->out_off);
            return -1;
        } else if (w < 0 && poll_reported_writable) {
            printf("SRV SPIN-WRITE fd=%d — POLLOUT then EAGAIN "
                   "(level bug)\n", cn->fd);
        }
    }
    return 0;
}

static void srv_drop(struct conn *cn, int ep)
{
    if (ep >= 0) epoll_ctl(ep, EPOLL_CTL_DEL, cn->fd, NULL);
    close(cn->fd);
    cn->fd = -1;
    srv_compact();
}

static int server_main(void)
{
    int ls = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un a; socklen_t alen;
    mk_addr(&a, &alen);
    unlink(SOCK_PATH);
    if (bind(ls, (struct sockaddr *)&a, alen) < 0) {
        printf("SRV bind FAIL %s\n", strerror(errno));
        return 1;
    }
    if (listen(ls, 32) < 0) {
        printf("SRV listen FAIL %s\n", strerror(errno));
        return 1;
    }
    fcntl(ls, F_SETFL, O_NONBLOCK);
    conns = calloc(SRV_MAXFDS, sizeof *conns);
    if (!conns) {
        printf("SRV arena alloc FAIL\n");
        return 1;
    }
    for (int i = 0; i < SRV_MAXFDS; i++) conns[i].fd = -1;
    nconn = 0;
    total_frames = accepts = 0;

    int ep = -1;
    if (use_et) {
        ep = epoll_create1(0);
        if (ep < 0) {
            printf("SRV epoll_create FAIL %s\n", strerror(errno));
            return 1;
        }
        struct epoll_event ev = { .events = EPOLLIN | EPOLLET,
                                  .data.fd = -1 };
        epoll_ctl(ep, EPOLL_CTL_ADD, ls, &ev);
        printf("SRV up %s ET maxevents=%d (Xorg shape)\n",
               use_abstract ? "abstract" : SOCK_PATH, ET_MAXEV);
    } else {
        printf("SRV up (%s) ls=%d poll\n",
               use_abstract ? "abstract" : SOCK_PATH, ls);
    }

    double t0 = mono(), last_hb = t0;

    if (use_et) {
        /* ---- epoll + EPOLLET driver (Xorg dispatch shape) ---- */
        struct epoll_event evs[ET_MAXEV];
        for (;;) {
            double now = mono();
            if (now - t0 > duration + 60) break;
            if (now - last_hb > 10.0) {
                last_hb = now;
                printf("SRV alive t=%.0f conns=%d frames=%u accepts=%u\n",
                       now - t0, nconn, total_frames, accepts);
            }
            int n = epoll_wait(ep, evs, ET_MAXEV, 1000);
            if (n < 0) {
                if (errno == EINTR) continue;
                printf("SRV epoll_wait FAIL %s\n", strerror(errno));
                break;
            }
            for (int k = 0; k < n; k++) {
                int fd = evs[k].data.fd;
                if (fd == -1) {
                    srv_accept(ls, ep);
                    continue;
                }
                struct conn *cn = srv_find(fd);
                if (!cn) continue;
                int dead = 0;
                if (evs[k].events & (EPOLLIN | EPOLLHUP | EPOLLERR))
                    dead = srv_readable(cn) < 0;
                if (!dead && (evs[k].events & (EPOLLOUT | EPOLLERR)))
                    dead = srv_writable(cn, evs[k].events & EPOLLOUT) < 0;
                if (dead) {
                    printf("SRV conn closed fd=%d (frames=%u) live=%d\n",
                           fd, cn->frames, nconn - 1);
                    srv_drop(cn, ep);
                    continue;
                }
                /* re-arm: EPOLLOUT only while echoes are pending */
                uint32_t want = EPOLLIN | EPOLLET;
                if (cn->out_len > cn->out_off) want |= EPOLLOUT;
                struct epoll_event ev = { .events = want, .data.fd = fd };
                epoll_ctl(ep, EPOLL_CTL_MOD, fd, &ev);
            }
        }
    } else {
        /* ---- poll driver (GLib / dbus-daemon shape) ---- */
        for (;;) {
            double now = mono();
            if (now - t0 > duration + 60) break;
            if (now - last_hb > 10.0) {
                last_hb = now;
                printf("SRV alive t=%.0f conns=%d frames=%u accepts=%u\n",
                       now - t0, nconn, total_frames, accepts);
            }
            int stalling = 0;
            {
                int el = (int)(now - t0);
                if (el >= STALL_EVERY && (el % STALL_EVERY) < STALL_FOR)
                    stalling = 1; /* silence reads: backpressure wave */
            }
            struct pollfd pf[SRV_MAXFDS + 1];
            int map[SRV_MAXFDS + 1];
            int n = 0;
            pf[n].fd = ls; pf[n].events = POLLIN; map[n] = -1; n++;
            for (int i = 0; i < nconn; i++) {
                short ev = 0;
                if (!stalling) ev |= POLLIN;
                if (conns[i].out_len > conns[i].out_off) ev |= POLLOUT;
                pf[n].fd = conns[i].fd; pf[n].events = ev; map[n] = i; n++;
            }
            int pr = poll(pf, n, 1000);
            if (pr < 0) {
                if (errno == EINTR) continue;
                printf("SRV poll FAIL %s\n", strerror(errno));
                break;
            }
            if (pr == 0) {
                srv_compact();
                continue;
            }
            for (int k = 0; k < n && pr > 0; k++) {
                if (!pf[k].revents) continue;
                pr--;
                if (map[k] == -1) {
                    srv_accept(ls, -1);
                    continue;
                }
                struct conn *cn = &conns[map[k]];
                int dead = 0;
                if (pf[k].revents & POLLIN)
                    dead = srv_readable(cn) < 0;
                if (!dead && pf[k].revents & (POLLOUT | POLLERR | POLLHUP))
                    dead = srv_writable(cn, pf[k].revents & POLLOUT) < 0;
                if (dead) {
                    printf("SRV conn closed fd=%d (frames=%u) live=%d\n",
                           cn->fd, cn->frames, nconn - 1);
                    srv_drop(cn, -1);
                }
            }
        }
    }
    printf("SRV done frames=%u accepts=%u conns=%d\n", total_frames, accepts,
           nconn);
    for (int i = 0; i < nconn; i++) close(conns[i].fd);
    close(ls);
    unlink(SOCK_PATH);
    return 0;
}

/* ============================================================
 * Client
 * ==========================================================*/
static int client_main(int cid)
{
    unsigned seed = (unsigned)(cid * 7919 + 13);
    struct sockaddr_un a; socklen_t alen;
    mk_addr(&a, &alen);
    unsigned seq = 0, errs = 0;
    unsigned long long tx = 0, rx = 0;
    double t0 = mono(), last_hb = t0;
    int fd = -1;
    unsigned char *req = malloc(128 * 1024);

    while (mono() - t0 < (double)duration) {
        double now = mono();
        if (now - last_hb > 10.0) {
            last_hb = now;
            printf("C%d alive t=%.0f seq=%u tx=%llu rx=%llu errs=%u\n", cid,
                   now - t0, seq, tx, rx, errs);
        }

        if (fd < 0) {
            fd = socket(AF_UNIX, SOCK_STREAM, 0);
            if (connect(fd, (struct sockaddr *)&a, alen) < 0) {
                printf("C%d connect FAIL %s\n", cid, strerror(errno));
                tsleep(0.5);
                close(fd); fd = -1;
                errs++;
                continue;
            }
        }

        /* build request */
        unsigned r = (unsigned)rand_r(&seed);
        unsigned len;
        if (r % 100 < 70)      len = 64 + r % 448;
        else if (r % 100 < 95) len = 512 + r % 7680;
        else                   len = 8192 + r % 57344;
        seq++;
        fill_payload(req + HDR_LEN, seq, len);
        unsigned crc = crc32_buf(req + HDR_LEN, len);
        put_u32(req, MAGIC0);
        put_u32(req + 4, (unsigned)cid);
        put_u32(req + 8, seq);
        put_u32(req + 12, len);
        put_u32(req + 16, crc);

        int e;
        double lat0 = mono();
        if (write_all(fd, req, HDR_LEN + len, &e) < 0) {
            if (e == EPIPE) {
                printf("C%d EPIPE-WRITE at seq=%u (server alive?!) t=%.0f\n",
                       cid, seq, mono() - t0);
                dump_netunix();
                _exit(5);
            }
            printf("C%d write FAIL errno=%d (%s) seq=%u\n", cid, e,
                   strerror(e), seq);
            close(fd); fd = -1; errs++;
            continue;
        }
        tx += HDR_LEN + len;

        /* read reply: hdr + payload */
        unsigned char rhdr[HDR_LEN];
        int rr = read_exact(fd, rhdr, HDR_LEN, &e);
        if (rr <= 0) {
            if (rr == 0) {
                printf("C%d EOF-UNEXPECTED at seq=%u t=%.0f — read()==0 "
                       "while server alive\n", cid, seq, mono() - t0);
                dump_netunix();
                _exit(6);
            }
            if (e == ETIMEDOUT) {
                printf("C%d WEDGE at seq=%u t=%.0f — no reply for 30s "
                       "(sent %u bytes)\n", cid, seq, mono() - t0, HDR_LEN + len);
                dump_netunix();
                _exit(3);
            }
            printf("C%d read-hdr FAIL errno=%d (%s) rc=%d seq=%u\n", cid, e,
                   strerror(e), rr, seq);
            close(fd); fd = -1; errs++;
            continue;
        }
        unsigned rmagic = get_u32(rhdr),
                 rseq = get_u32(rhdr + 8), rlen = get_u32(rhdr + 12),
                 rcrc = get_u32(rhdr + 16);
        if (rlen > 128 * 1024) {
            printf("C%d BOGUS reply len=%u seq=%u\n", cid, rlen, rseq);
            dump_netunix();
            _exit(4);
        }
        rr = read_exact(fd, req, rlen, &e);
        if (rr <= 0) {
            if (rr == 0) {
                printf("C%d EOF-UNEXPECTED(body) seq=%u\n", cid, rseq);
                dump_netunix();
                _exit(6);
            }
            if (e == ETIMEDOUT) {
                printf("C%d WEDGE(body) seq=%u\n", cid, rseq);
                dump_netunix();
                _exit(3);
            }
            printf("C%d read-body FAIL errno=%d rc=%d\n", cid, e, rr);
            close(fd); fd = -1; errs++;
            continue;
        }
        rx += HDR_LEN + rlen;
        int bad = 0;
        const char *why = "";
        if (rmagic != MAGIC0) { bad = 1; why = "magic"; }
        else if (rseq != seq) { bad = 1; why = "seq-mismatch"; }
        else if (rcrc != crc32_buf(req, rlen) || rlen != len) {
            bad = 1; why = "crc/len";
        }
        if (bad) {
            printf("C%d DATA-ERR (%s) got {seq=%u len=%u} want {seq=%u "
                   "len=%u}\n", cid, why, rseq, rlen, seq, len);
            dump_netunix();
            _exit(4);
        }
        double lat = mono() - lat0;
        if (lat > 5.0)
            printf("C%d SLOW %.2fs seq=%u len=%u\n", cid, lat, seq, len);

        /* vary pacing: mostly full speed, sometimes a short pause */
        if (r % 10 == 0) tsleep(0.002);
    }

    /* graceful end: shutdown write, expect echo of nothing + EOF */
    shutdown(fd, SHUT_WR);
    unsigned char b[64];
    for (;;) {
        ssize_t rd = read(fd, b, sizeof b);
        if (rd < 0 && errno == EINTR) continue;
        if (rd <= 0) break;
    }
    close(fd);
    printf("C%d DONE seq=%u tx=%llu rx=%llu errs=%u\n", cid, seq, tx, rx,
           errs);
    free(req);
    return errs ? 1 : 0;
}

/* ============================================================
 * Chaos client: connect, few exchanges, abrupt close mid-stream
 * ==========================================================*/
static int chaos_main(void)
{
    struct sockaddr_un a; socklen_t alen;
    mk_addr(&a, &alen);
    unsigned rnd = 999;
    double t0 = mono();
    unsigned cycles = 0, hangs = 0;
    while (mono() - t0 < (double)duration) {
        int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK, 0);
        int cnn = connect(fd, (struct sockaddr *)&a, alen);
        if (cnn == 0 || (cnn < 0 && (errno == EAGAIN || errno == EINPROGRESS))) {
            if (cnn < 0) {
                /* backlog full: wait for room, watchdog-bounded */
                struct pollfd pf = { .fd = fd, .events = POLLOUT };
                if (poll(&pf, 1, 10000) != 1) {
                    printf("CHAOS connect-wedge (10s) cycle=%u\n", cycles);
                    hangs++;
                    close(fd);
                    tsleep(1.0);
                    continue;
                }
            }
            cycles++;
            /* send a couple of frames, read none, close abruptly */
            unsigned char buf[HDR_LEN + 256];
            for (int i = 0; i < 3; i++) {
                unsigned seq = cycles * 100 + (unsigned)i;
                fill_payload(buf + HDR_LEN, seq, 256);
                put_u32(buf, MAGIC0);
                put_u32(buf + 4, 0xC0A05u);
                put_u32(buf + 8, seq);
                put_u32(buf + 12, 256);
                put_u32(buf + 16, crc32_buf(buf + HDR_LEN, 256));
                int e;
                write_all(fd, buf, sizeof buf, &e);
                /* sometimes read a bit (watchdog-bounded), mostly not */
                if (rand_r(&rnd) % 3 == 0) {
                    struct pollfd pr = { .fd = fd, .events = POLLIN };
                    if (poll(&pr, 1, 5000) == 1) {
                        unsigned char rb[128];
                        (void)!read(fd, rb, sizeof rb);
                    }
                }
            }
            close(fd); /* RST-like: pending server replies dropped */
        } else if (cnn < 0 && errno != ECONNREFUSED) {
            printf("CHAOS connect FAIL %s\n", strerror(errno));
        }
        tsleep(1.0 + (rand_r(&rnd) % 300) / 100.0);
    }
    printf("CHAOS DONE cycles=%u hangs=%u\n", cycles, hangs);
    return hangs ? 1 : 0;
}

/* ============================================================
 * Supervisor
 * ==========================================================*/
int main(int argc, char **argv)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    signal(SIGCHLD, SIG_DFL);
    signal(SIGPIPE, SIG_IGN); /* like dbus-daemon: EPIPE, not death */
    if (argc > 1 && !strcmp(argv[1], "srv-only")) {
        duration = argc > 2 ? atoi(argv[2]) : 30;
        if (argc > 3 && !strcmp(argv[3], "abs")) use_abstract = 1;
        if (argc > 3 && !strcmp(argv[3], "et")) use_et = 1;
        return server_main();
    }
    if (argc > 1 && !strcmp(argv[1], "chaos-only")) {
        duration = argc > 2 ? atoi(argv[2]) : 30;
        if (argc > 3 && !strcmp(argv[3], "abs")) use_abstract = 1;
        return chaos_main();
    }
    if (argc > 1 && !strcmp(argv[1], "cli-only")) {
        int cid = argc > 2 ? atoi(argv[2]) : 1;
        duration = argc > 3 ? atoi(argv[3]) : 10;
        if (argc > 4 && !strcmp(argv[4], "abs")) use_abstract = 1;
        return client_main(cid);
    }
    if (argc > 1) duration = atoi(argv[1]);
    if (argc > 2) nclients = atoi(argv[2]);
    for (int i = 3; i < argc && i < 5; i++) {
        if (!strcmp(argv[i], "abs")) use_abstract = 1;
        if (!strcmp(argv[i], "et")) use_et = 1;
    }
    if (nclients > MAX_CLIENTS) nclients = MAX_CLIENTS;

    mkdir("/proc", 0755);
    mount("proc", "/proc", "proc", 0, NULL);

    printf("UNIXSTRESS begin dur=%ds clients=%d %s\n", duration, nclients,
           use_abstract ? "abstract" : SOCK_PATH);

    int p0 = phase0();
    printf("P0 %s\n", p0 ? "FAIL (continuing to load phase)" : "PASS");
    int p0_fail = p0 != 0;

    unlink(SOCK_PATH);
    pid_t srv = fork();
    if (srv == 0) _exit(server_main());
    tsleep(0.5); /* let the listener come up */

    pid_t pids[MAX_CLIENTS + 1];
    int kinds[MAX_CLIENTS + 1]; /* 0=client 1=chaos */
    int np = 0;
    for (int i = 1; i <= nclients; i++) {
        pid_t p = fork();
        if (p == 0) _exit(client_main(i));
        pids[np] = p; kinds[np] = 0; np++;
    }
    pid_t ch = fork();
    if (ch == 0) _exit(chaos_main());
    pids[np] = ch; kinds[np] = 1; np++;

    printf("SUPERV server=%d clients=%d chaos=%d\n", srv, nclients, ch);

    int fail = 0;
    int finished = 0;
    double t0 = mono();
    while (finished < np) {
        int st;
        pid_t p = waitpid(-1, &st, WNOHANG);
        if (p < 0) break;
        if (p == 0) {
            tsleep(2.0);
            if (mono() - t0 > duration + 300) {
                printf("SUPERV TIMEOUT — killing stragglers\n");
                for (int i = 0; i < np; i++) kill(pids[i], SIGKILL);
                kill(srv, SIGKILL);
                fail = 1;
                break;
            }
            continue;
        }
        for (int i = 0; i < np; i++) {
            if (pids[i] == p) {
                int code = WIFEXITED(st) ? WEXITSTATUS(st) : 128 +
                           (WIFSIGNALED(st) ? WTERMSIG(st) : 0);
                const char *why = "ok";
                if (kinds[i] == 1)
                    why = code == 0 ? "chaos-ok" : "chaos-fail";
                else if (code == 3) { why = "WEDGE"; fail = 1; }
                else if (code == 4) { why = "DATA-ERR"; fail = 1; }
                else if (code == 5) { why = "EPIPE"; fail = 1; }
                else if (code == 6) { why = "EOF-UNEXPECTED"; fail = 1; }
                else if (code != 0) { why = "client-err"; fail = 1; }
                printf("SUPERV child %d exit=%d (%s)\n", p, code, why);
                finished++;
                break;
            }
        }
    }
    /* reap or kill the server */
    kill(srv, SIGTERM);
    for (double w = mono(); mono() - w < 10.0;) {
        int st;
        pid_t p = waitpid(srv, &st, WNOHANG);
        if (p == srv) break;
        if (p == 0) { tsleep(0.5); continue; }
    }
    kill(srv, SIGKILL);
    waitpid(srv, NULL, 0);

    int verdict_fail = fail || p0_fail;
    printf("UNIXSTRESS end VERDICT:%s\n",
           verdict_fail ? "FAIL" : "PASS");
    return verdict_fail ? 1 : 0;
}
