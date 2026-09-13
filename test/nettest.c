/* nettest — freestanding loopback network E2E test for Rux (Wave 3 acceptance).
 *
 * Raw syscalls only (no libc). Exercises the full stack:
 *   UDP:  bind(127.0.0.1:P) → sendto(same) → recvfrom
 *   TCP:  listen → connect → accept → send → recv (echo)
 * Prints "NETTEST PASS"/"NETTEST FAIL: <n>" and exits.
 */
typedef unsigned long u64;
typedef long s64;
typedef unsigned int u32;
typedef unsigned short u16;

#define __NR_write 64
#define __NR_read 63
#define __NR_write 64
#define __NR_lseek 62
#define O_TRUNC 0x200
#define __NR_exit 93
#define __NR_exit_group 94
#define __NR_nanosleep 35
#define __NR_socket 198
#define __NR_bind 200
#define __NR_listen 201
#define __NR_accept 202
#define __NR_connect 203
#define __NR_sendto 206
#define __NR_recvfrom 207

#define AF_INET 2
#define SOCK_STREAM 1
#define SOCK_DGRAM 2

static s64 sys3(s64 n, s64 a, s64 b, s64 c)
{
    register s64 a0 asm("a0") = a;
    register s64 a1 asm("a1") = b;
    register s64 a2 asm("a2") = c;
    register s64 a7 asm("a7") = n;
    asm volatile("ecall"
                 : "+r"(a0)
                 : "r"(a1), "r"(a2), "r"(a7)
                 : "memory");
    return a0;
}

static s64 sys6(s64 n, s64 a, s64 b, s64 c, s64 d, s64 e, s64 f)
{
    register s64 a0 asm("a0") = a;
    register s64 a1 asm("a1") = b;
    register s64 a2 asm("a2") = c;
    register s64 a3 asm("a3") = d;
    register s64 a4 asm("a4") = e;
    register s64 a5 asm("a5") = f;
    register s64 a7 asm("a7") = n;
    asm volatile("ecall"
                 : "+r"(a0)
                 : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a5), "r"(a7)
                 : "memory");
    return a0;
}

struct sockaddr_in {
    u16 family;
    u16 port;    /* network order */
    u32 addr;    /* network order */
    unsigned char zero[8];
};

static void msleep(int ms)
{
    /* nananosleep(timespec {tv_sec, tv_nsec}, NULL) */
    u64 ts[2];
    ts[0] = ms / 1000;
    ts[1] = (ms % 1000) * 1000000L;
    sys3(__NR_nanosleep, (s64)ts, 0, 0);
}

static void puts_(const char *s)
{
    int len = 0;
    while (s[len])
        len++;
    sys3(__NR_write, 1, (s64)s, len);
}

static void sock_setup(struct sockaddr_in *sa, u16 port_be, u32 addr_be)
{
    int i;
    sa->family = AF_INET;
    sa->port = port_be;
    sa->addr = addr_be;
    for (i = 0; i < 8; i++)
        sa->zero[i] = 0;
}

#define UDP_PORT 0x983A /* 15000 in big-endian bytes (0x3A98) */
#define TCP_PORT 0x983B /* 15001 */
#define LOOPBACK 0x0100007Fu /* 127.0.0.1 big-endian */

static int udp_test(void)
{
    struct sockaddr_in sa;
    s64 fd = sys3(__NR_socket, AF_INET, SOCK_DGRAM, 0);
    char msg[8] = "udp-ping";
    char buf[64];

    if (fd < 0)
        return 1;
    sock_setup(&sa, UDP_PORT, LOOPBACK);
    if (sys3(__NR_bind, fd, (s64)&sa, 16) < 0)
        return 2;
    if (sys6(__NR_sendto, fd, (s64)msg, 8, 0, (s64)&sa, 16) != 8)
        return 3;

    /* The loopback backlog drains in the NetRx softirq (timer tick at
     * HZ=100): retry for up to ~2s. */
    for (int i = 0; i < 200; i++) {
        s64 n = sys6(__NR_recvfrom, fd, (s64)buf, sizeof(buf), 0, 0, 0);
        if (n >= 8) {
            for (int j = 0; j < 8; j++)
                if (buf[j] != msg[j])
                    return 4;
            return 0;
        }
        if (n != -11) /* not EAGAIN */
            return 5;
        msleep(10);
    }
    return 6;
}

static int tcp_test(void)
{
    struct sockaddr_in sa;
    s64 lfd, cfd, afd;
    char msg[8] = "tcp-ping";
    char buf[64];

    lfd = sys3(__NR_socket, AF_INET, SOCK_STREAM, 0);
    cfd = sys3(__NR_socket, AF_INET, SOCK_STREAM, 0);
    if (lfd < 0 || cfd < 0)
        return 11;

    sock_setup(&sa, TCP_PORT, LOOPBACK);
    if (sys3(__NR_bind, lfd, (s64)&sa, 16) < 0)
        return 12;
    if (sys3(__NR_listen, lfd, 4, 0) < 0)
        return 13;

    /* connect sends the SYN; loopback delivery is async */
    if (sys3(__NR_connect, cfd, (s64)&sa, 16) < 0)
        return 14;

    /* Poll accept until the handshake completes (softirq driven). */
    afd = -11;
    for (int i = 0; i < 300; i++) {
        afd = sys3(__NR_accept, lfd, 0, 0);
        if (afd >= 0)
            break;
        msleep(10);
    }
    if (afd < 0)
        return 15;

    if (sys6(__NR_sendto, cfd, (s64)msg, 8, 0, 0, 0) != 8)
        return 16;

    for (int i = 0; i < 200; i++) {
        s64 n = sys6(__NR_recvfrom, afd, (s64)buf, sizeof(buf), 0, 0, 0);
        if (n >= 8) {
            for (int j = 0; j < 8; j++)
                if (buf[j] != msg[j])
                    return 17;
            return 0;
        }
        if (n != -11)
            return 18;
        msleep(10);
    }
    return 19;
}

#define __NR_openat 56
#define __NR_close 57
#define __NR_unlinkat 35
#define O_CREAT 0x40
#define O_WRONLY 2
#define O_RDONLY 0

static int file_test(void)
{
    static const char path[] = "/tmp/nettest_f1\0";
    char msg[16] = "0123456789ABCDEF";
    char buf[16];
    s64 fd, nr, pos;

    /* two create+delete cycles first: forces inode-number reuse, which is
     * what smoke_test's earlier cases set up before its lseek check */
    for (int i = 0; i < 2; i++) {
        fd = sys6(__NR_openat, -100, (s64)path, O_CREAT | O_WRONLY | O_TRUNC, 0600, 0, 0);
        if (fd < 0) return 40;
        sys3(__NR_write, fd, (s64)msg, 16);
        sys3(__NR_close, fd, 0, 0);
        sys3(__NR_unlinkat, -100, (s64)path, 0);
    }

    fd = sys6(__NR_openat, -100, (s64)path, O_CREAT | O_WRONLY | O_TRUNC, 0600, 0, 0);
    if (fd < 0) return 41;
    s64 w = sys3(__NR_write, fd, (s64)msg, 16);
    sys3(__NR_close, fd, 0, 0);
    if (w != 16) return 42;

    fd = sys6(__NR_openat, -100, (s64)path, O_RDONLY, 0, 0, 0);
    if (fd < 0) return 43;

    /* read at offset 6 via seek */
    pos = sys3(__NR_lseek, fd, 6, 0);
    if (pos != 6) return 44;
    for (int i = 0; i < 8; i++) buf[i] = '.';
    nr = sys3(__NR_read, fd, (s64)buf, 8);
    sys3(__NR_close, fd, 0, 0);
    sys3(__NR_unlinkat, -100, (s64)path, 0);

    /* report: n=<nr> data=<8 bytes hex> */
    puts_("filetest nr=");
    for (int i = 0; i < 8; i++) {
        char hex[3];
        unsigned char c = buf[i];
        hex[0] = "0123456789abcdef"[c >> 4];
        hex[1] = "0123456789abcdef"[c & 0xf];
        hex[2] = 0;
        puts_(hex);
    }
    puts_("\n");
    if (nr == 8) {
        for (int i = 0; i < 8; i++)
            if (buf[i] != msg[6 + i])
                return 45; /* content mismatch */
        return 0;
    }
    return 46; /* short/failed read */
}

#define __NR_ftruncate 46

/* truncate-to-8 must keep the first 8 bytes (kernel write/truncate/cache
 * coherence — used to return stale/zero content after inode churn) */
static int trunc_test(void)
{
    static const char path[] = "/tmp/nettest_t1\0";
    char msg[16] = "0123456789ABCDEF";
    char buf[8];
    s64 fd, nr;

    for (int i = 0; i < 2; i++) {
        fd = sys6(__NR_openat, -100, (s64)path, O_CREAT | O_WRONLY | O_TRUNC, 0600, 0, 0);
        if (fd < 0) return 50;
        sys3(__NR_write, fd, (s64)msg, 16);
        sys3(__NR_close, fd, 0, 0);
        sys3(__NR_unlinkat, -100, (s64)path, 0);
    }

    fd = sys6(__NR_openat, -100, (s64)path, O_CREAT | O_WRONLY | O_TRUNC, 0600, 0, 0);
    if (fd < 0) return 51;
    if (sys3(__NR_write, fd, (s64)msg, 16) != 16) return 52;
    if (sys3(__NR_ftruncate, fd, 8, 0) != 0) return 53;
    sys3(__NR_close, fd, 0, 0);

    fd = sys6(__NR_openat, -100, (s64)path, O_RDONLY, 0, 0, 0);
    if (fd < 0) return 54;
    nr = sys3(__NR_read, fd, (s64)buf, 8);
    sys3(__NR_close, fd, 0, 0);
    sys3(__NR_unlinkat, -100, (s64)path, 0);
    if (nr != 8) return 55;
    for (int i = 0; i < 8; i++)
        if (buf[i] != msg[i]) return 56;
    return 0;
}

#define __NR_kill 129
#define __NR_rt_sigaction 134
#define __NR_rt_sigprocmask 135
#define __NR_rt_sigtimedwait 137
#define __NR_getpid 172
#define SIGUSR2 12
#define SA_RESTORER 0x04000000

/* ABI + sigwait: rt_sigaction 32-byte layout round-trip, sigtimedwait
 * consumes a pending blocked signal, zero timeout → EAGAIN */
static int sig_test(void)
{
    /* 1. sigaction set/query round-trip (32-byte user ABI) */
    u64 act[4] = { 1 /*SIG_IGN*/, SA_RESTORER, 0x1234 /*restorer*/, 0x000000ff /*mask*/ };
    u64 old[4] = { 0, 0, 0, 0 };
    s64 ret = sys6(__NR_rt_sigaction, SIGUSR2, (s64)act, (s64)old, 8, 0, 0);
    if (ret != 0) return 60;
    /* install again with a different mask; oldact must return the first */
    u64 act2[4] = { 0 /*SIG_DFL — pending must stick for sigwait*/, 0, 0, 0x0000f000 };
    ret = sys6(__NR_rt_sigaction, SIGUSR2, (s64)act2, (s64)old, 8, 0, 0);
    if (ret != 0) return 61;
    if (old[0] != 1) return 62;            /* handler */
    if (old[3] != 0x000000ff) return 63;   /* sa_mask at offset 24 (ABI) */

    /* 2. block SIGUSR2, self-send, sigtimedwait must consume it */
    u64 blk = 1u << (SIGUSR2 - 1);
    ret = sys6(__NR_rt_sigprocmask, 0 /*SIG_BLOCK*/, (s64)&blk, (s64)&old[0], 8, 0, 0);
    if (ret != 0) return 64;
    ret = sys3(__NR_kill, sys3(__NR_getpid, 0, 0, 0), SIGUSR2, 0);
    if (ret != 0) return 65;

    u64 waitset = blk;
    u64 ts[2] = { 0, 0 };
    ret = sys6(__NR_rt_sigtimedwait, (s64)&waitset, 0, (s64)&ts, 8, 0, 0);
    if (ret != SIGUSR2) return 66;

    /* 3. consumed → zero timeout must be EAGAIN (-11) */
    ret = sys6(__NR_rt_sigtimedwait, (s64)&waitset, 0, (s64)&ts, 8, 0, 0);
    if (ret != -11) return 67;

    /* restore ignore + unblock */
    u64 ign[4] = { 1, 0, 0, 0 };
    sys6(__NR_rt_sigaction, SIGUSR2, (s64)ign, 0, 8, 0, 0);
    u64 unblk = ~blk;
    sys6(__NR_rt_sigprocmask, 1 /*SIG_UNBLOCK*/, (s64)&blk, 0, 8, 0, 0);
    (void)unblk;
    return 0;
}

void _start(void)
{
    int r = sig_test();
    if (r == 0) {
        puts_("sig: abi+sigwait ok\n");
    } else {
        puts_("sig: FAIL\n");
    }
    r = file_test();
    if (r == 0) {
        puts_("file: readback ok\n");
        r = trunc_test();
        if (r == 0)
            puts_("trunc: readback ok\n");
    }
    if (r != 0) {
        puts_("file/trunc: FAIL\n");
    }
    r = udp_test();
    if (r == 0) {
        puts_("udp: echo ok\n");
        r = tcp_test();
    }
    if (r == 0)
        puts_("tcp: echo ok\nNETTEST PASS\n");
    else {
        char buf[24];
        puts_("NETTEST FAIL code=");
        int i = 0;
        if (r < 0) { buf[i++] = '-'; r = -r; }
        char tmp[12];
        int n = 0;
        do { tmp[n++] = '0' + (r % 10); r /= 10; } while (r);
        while (n) buf[i++] = tmp[--n];
        buf[i++] = '\n';
        buf[i] = 0;
        puts_(buf);
    }
    sys3(__NR_exit_group, r == 0 ? 0 : r, 0, 0);
    for (;;)
        ;
}
