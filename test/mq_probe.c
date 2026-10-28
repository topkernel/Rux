/* mq_probe.c — raw-syscall POSIX mqueue timeout probe (no libc).
 * riscv64: mq_open=240 mq_timedsend=242 mq_timedreceive=243
 * clock_gettime=169 close=57
 *
 * 1) print CLOCK_REALTIME
 * 2) mq_open with maxmsg=2, msgsize=64
 * 3) fill the queue (2 sends, must succeed instantly)
 * 4) mq_timedsend with ABSOLUTE deadline now+3s -> expect rc=-1
 *    errno=ETIMEDOUT after ~3 seconds
 * 5) print elapsed realtime to prove the timeout fired on schedule
 */
typedef unsigned long u64;
typedef long i64;

static inline i64 sys3(i64 n, i64 a, i64 b, i64 c)
{
    register i64 a0 asm("a0") = a;
    register i64 a1 asm("a1") = b;
    register i64 a2 asm("a2") = c;
    register i64 a7 asm("a7") = n;
    asm volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a7) : "memory");
    return a0;
}
static inline i64 sys5(i64 n, i64 a, i64 b, i64 c, i64 d, i64 e)
{
    register i64 a0 asm("a0") = a;
    register i64 a1 asm("a1") = b;
    register i64 a2 asm("a2") = c;
    register i64 a3 asm("a3") = d;
    register i64 a4 asm("a4") = e;
    register i64 a7 asm("a7") = n;
    asm volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a7) : "memory");
    return a0;
}

struct ts { i64 sec; i64 nsec; };
struct mq_attr { i64 flags, maxmsg, msgsize, curmsgs, rsv[4]; };

static void puts_(const char *s)
{
    u64 n = 0;
    while (s[n]) n++;
    sys3(64, 1, (i64)s, (i64)n);
}
static void putd(i64 v)
{
    char b[24], r[24];
    int n = 0;
    if (v < 0) { sys3(64, 1, (i64)"-", 1); v = -v; }
    if (v == 0) b[n++] = '0';
    while (v > 0) { b[n++] = '0' + (v % 10); v /= 10; }
    for (int i = 0; i < n; i++) r[i] = b[n - 1 - i];
    sys3(64, 1, (i64)r, n);
    sys3(64, 1, (i64)"\n", 1);
}

static i64 rt_now(struct ts *t)
{
    return sys3(169, 0 /*CLOCK_REALTIME*/, (i64)t, 0);
}

void _start(void)
{
    struct ts t0, dl;
    puts_("MQ-PROBE-START");
    rt_now(&t0);
    puts_("realtime_sec:");
    putd(t0.sec);

    struct mq_attr attr = { 0, 2, 64, 0, {0,0,0,0} };
    i64 fd = sys5(180 /*mq_open*/, (i64)"/mqprobe", 0x42 /*O_CREAT|O_RDWR*/,
                  0644, (i64)&attr, 0);
    puts_("mq_open fd:");
    putd(fd);
    if (fd < 0) goto out;

    /* fill: exactly maxmsg sends */
    for (int i = 0; i < 2; i++) {
        i64 r = sys5(182, fd, (i64)"msg0000", 7, 0, 0 /*NULL timeout*/);
        puts_("fill send rc:");
        putd(r);
        if (r != 0) goto out;
    }

    /* check mq_getsetattr reports the true curmsgs */
    {
        struct mq_attr out = { 0,0,0,0, {0,0,0,0} };
        i64 g = sys5(185 /*mq_getsetattr*/, fd, 0, (i64)&out, 0, 0);
        puts_("getattr rc:");
        putd(g);
        puts_("getattr curmsgs:");
        putd(out.curmsgs);
        puts_("getattr maxmsg:");
        putd(out.maxmsg);
        puts_("getattr msgsize:");
        putd(out.msgsize);
    }

    /* absolute deadline: now + 3s */
    rt_now(&dl);
    dl.sec += 3;
    puts_("timedsend deadline:");
    putd(dl.sec);

    i64 r = sys5(182, fd, (i64)"blocked", 7, 0, (i64)&dl);
    puts_("timedsend rc:");
    putd(r);

    struct ts t1;
    rt_now(&t1);
    puts_("realtime_after:");
    putd(t1.sec);
    puts_("elapsed_us:");
    putd((t1.sec - t0.sec) * 1000000 + (t1.nsec - t0.nsec) / 1000);

    sys3(57, fd, 0, 0);
    sys3(181 /*mq_unlink*/, (i64)"/mqprobe", 0, 0);
out:
    puts_("MQ-PROBE-DONE");
    sys3(93, 0, 0, 0);
    __builtin_unreachable();
}
