/* slirp_test — real virtio-net traffic E2E via QEMU user networking (R34).
 *
 * Freestanding, raw-syscall style (same build recipe as nettest.c).
 * Unlike nettest (whose traffic takes the 127.0.0.1 loopback
 * short-circuit in ethernet_send), every packet here crosses the real
 * virtio-net MMIO device and the QEMU slirp backend:
 *
 *   ARP:  the first send to 10.0.2.x finds no cache entry → broadcast
 *         ARP request; slirp replies → unicast retrys succeed.
 *   TX:   virtio-net transmit queue (queue 1) hdr+data chains must
 *         complete through the used ring (no wait_for_completion hang).
 *   RX:   virtio-net receive queue (queue 0) posted buffers, used-ring
 *         consumption, phys_to_virt conversion of the DMA address.
 *   UDP:  DNS query to 10.0.2.3:53 — slirp's built-in resolver answers,
 *         so the round-trip needs no host service.
 *   TCP:  connect to 10.0.2.2:80 (gateway → host loopback). Without a
 *         host listener the expected outcome is SYN out / RST back;
 *         any of {data, clean FIN, ECONNRESET} proves the full path.
 *
 * Exit 0 iff the UDP DNS round-trip succeeded. TCP outcome is printed
 * but only downgrades the verdict to "DEGRADED" (never fails alone).
 */
typedef unsigned long u64;
typedef long s64;
typedef unsigned int u32;
typedef unsigned short u16;
typedef unsigned char u8;

#define __NR_write 64
#define __NR_exit_group 94
#define __NR_nanosleep 101
#define __NR_socket 198
#define __NR_bind 200
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

static void putdec_(long v)
{
    char buf[16];
    int i = 0, n = 0;
    char tmp[16];
    if (v < 0) { buf[i++] = '-'; v = -v; }
    do { tmp[n++] = '0' + (v % 10); v /= 10; } while (v);
    while (n) buf[i++] = tmp[--n];
    buf[i] = 0;
    puts_(buf);
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

/* sin_addr bytes must be a.b.c.d in memory order; on the LE target that
 * is the byte-swapped u32 (same convention as nettest's 127.0.0.1
 * constant 0x0100007F). */
#define LEIP(a, b, c, d) ((u32)(d) << 24 | (u32)(c) << 16 | (u32)(b) << 8 | (u32)(a))
#define SLIRP_DNS  LEIP(10, 0, 2, 3)   /* slirp virtual resolver  */
#define SLIRP_HOST LEIP(10, 0, 2, 2)   /* slirp gateway = host    */
#define LOCAL_ADDR LEIP(10, 0, 2, 15)  /* slirp guest address     */

/* sin_port/sin_addr must carry network-order BYTES in memory; on the LE
 * target that means byte-swapped u16/u32 values (nettest's 0x983A = 15000
 * convention — see its UDP_PORT). */
#define UDP_PORT 0xFC3A /* 15100 */
#define TCP_PORT 0x5000 /* 80 */
#define DNS_PORT 0x3500 /* 53 */

/* 21-byte minimal DNS query: id 0x1234, RD, one question "com A IN". */
static const unsigned char dns_query[] = {
    0x12, 0x34,                         /* id */
    0x01, 0x00,                         /* flags: recursion desired */
    0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x03, 'c', 'o', 'm', 0x00,          /* qname */
    0x00, 0x01,                         /* qtype = A */
    0x00, 0x01,                         /* qclass = IN */
};

/* UDP: send the DNS query to 10.0.2.3:53 until a matching reply lands.
 * Retries are required: the very first packet goes out while ARP is
 * still unresolved (broadcast Ethernet destination), and slirp's reply
 * only arms the cache for later unicast sends. */
static int udp_test(void)
{
    struct sockaddr_in local, remote;
    char buf[512];

    s64 fd = sys3(__NR_socket, AF_INET, SOCK_DGRAM, 0);
    if (fd < 0)
        return 1;

    sock_setup(&local, UDP_PORT, LOCAL_ADDR);
    if (sys3(__NR_bind, fd, (s64)&local, 16) < 0)
        return 2;

    sock_setup(&remote, DNS_PORT, SLIRP_DNS);

    for (int attempt = 1; attempt <= 30; attempt++) {
        s64 n = sys6(__NR_sendto, fd, (s64)dns_query, sizeof(dns_query),
                     0, (s64)&remote, 16);
        if (n != (s64)sizeof(dns_query))
            return 3;

        /* recvfrom drives ethernet_poll() in the kernel, so each retry
         * also pumps the RX path even without an interrupt. */
        for (int i = 0; i < 10; i++) {
            n = sys6(__NR_recvfrom, fd, (s64)buf, sizeof(buf), 0, 0, 0);
            if (n >= 12) {
                if (buf[0] == 0x12 && buf[1] == 0x34) {
                    puts_("  udp: reply id matches, ");
                    putdec_(n);
                    puts_(" bytes\n");
                    return 0;
                }
                puts_("  udp: reply id MISMATCH\n");
                return 4;
            }
            if (n != -11) { /* not EAGAIN */
                putdec_(n);
                puts_(" = recv err\n");
                return 5;
            }
            msleep(50);
        }
        puts_("  udp: attempt ");
        putdec_(attempt);
        puts_(" no reply yet\n");
    }
    return 6; /* nothing came back: ARP/TX/RX or resolver failed */
}

/* TCP: connect to the slirp gateway's port 80. Outcome classes:
 *   0  data received     (host had a listener)
 *   10 clean EOF/FIN
 *   11 ECONNRESET        (RST — host refused: expected, still proves path)
 *   12 timeout           (no reply at all — path broken)
 */
static int tcp_test(void)
{
    struct sockaddr_in remote;
    static const char get[] = "GET / HTTP/1.0\r\n\r\n";
    char buf[256];

    s64 fd = sys3(__NR_socket, AF_INET, SOCK_STREAM, 0);
    if (fd < 0)
        return 20;

    sock_setup(&remote, TCP_PORT, SLIRP_HOST);
    if (sys3(__NR_connect, fd, (s64)&remote, 16) < 0)
        return 21;

    for (int round = 0; round < 60; round++) {
        /* SYN needs a few retransmit ticks while ARP resolves. */
        sys6(__NR_sendto, fd, (s64)get, sizeof(get) - 1, 0, 0, 0);
        for (int i = 0; i < 10; i++) {
            s64 n = sys6(__NR_recvfrom, fd, (s64)buf, sizeof(buf), 0, 0, 0);
            if (n > 0) {
                puts_("  tcp: got ");
                putdec_(n);
                puts_(" bytes\n");
                return 0;
            }
            if (n == 0) {
                puts_("  tcp: connection closed by peer (FIN/RST)\n");
                return 10;
            }
            if (n == -104) { /* ECONNRESET */
                puts_("  tcp: RST from gateway (host refused)\n");
                return 11;
            }
            if (n != -11) {
                puts_("  tcp: recv ");
                putdec_(n);
                puts_("\n");
                return 22;
            }
            msleep(50);
        }
    }
    puts_("  tcp: no reply (timeout)\n");
    return 12;
}

void _start(void)
{
    int udp_rc, tcp_rc;

    puts_("SLIRP-START (virtio-net MMIO <-> slirp 10.0.2.0/24)\n");

    puts_("phase: udp dns 10.0.2.3:53\n");
    udp_rc = udp_test();
    if (udp_rc == 0) {
        puts_("SLIRP-UDP-PASS (ARP + TX + RX + UDP round-trip)\n");
    } else {
        puts_("SLIRP-UDP-FAIL code=");
        putdec_(udp_rc);
        puts_("\n");
    }

    puts_("phase: tcp connect 10.0.2.2:80\n");
    tcp_rc = tcp_test();

    if (udp_rc == 0 && tcp_rc != 12) {
        puts_("SLIRP PASS\n");
        sys3(__NR_exit_group, 0, 0, 0);
    } else if (udp_rc == 0) {
        /* UDP round-trip proved the datapath; TCP just got no answer. */
        puts_("SLIRP DEGRADED (udp ok, tcp timeout)\n");
        sys3(__NR_exit_group, 0, 0, 0);
    } else {
        puts_("SLIRP FAIL udp=");
        putdec_(udp_rc);
        puts_(" tcp=");
        putdec_(tcp_rc);
        puts_("\n");
        sys3(__NR_exit_group, udp_rc, 0, 0);
    }
    for (;;)
        ;
}
