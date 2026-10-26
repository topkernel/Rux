/*
 * schedrepro.c — standalone scheduler-starvation reproducer (leg-26).
 *
 * Builds the OH-boot starvation shapes on the minimal rootfs, no OH
 * userspace needed:
 *
 *   W  wake storm     — waker threads store a timestamp then futex_wake a
 *                       parked worker; the worker stamps on return. The
 *                       delta is the userspace-visible wake->run latency
 *                       (kernel wake path + pick + return to user).
 *   P  futex pong     — N pairs hopping a futex word; per-hop latency
 *                       (the samgr/handler wake-for-work shape).
 *   H  cpu hogs       — pure userspace compute spinners (fully
 *                       preemptible; monopolies can only come from the
 *                       kernel refusing to preempt).
 *   F  fork churn     — threads fork+waitpid in a tight loop (the OH
 *                       appspawn restart-loop shape: fresh tasks, pid
 *                       hash + pick pressure, exit storms).
 *   R  pipe req/rep   — N request/reply pairs over pipes (binder-shaped
 *                       synchronous IPC: two wakeups per round trip).
 *   D  watchdog       — 50 ms clock_nanosleep loop measuring overshoot;
 *                       any gap > 250 ms is logged with its timestamp,
 *                       max gap = the starvation verdict number.
 *
 * Config via argv[1], else `schedrepro=` on /proc/cmdline, else the
 * compiled default. Grammar (order free, ':' separated):
 *   W<workers>,x<wakers> P<pairs> H<hogs> F<churners> R<pairs> T<secs>
 *   B<kb-shared-btree?> (reserved)  — e.g. "W8,x2:P4:H2:F2:R4:T60"
 *
 * Output (serial): parsable SREPRO lines; final verdict line
 *   SREPRO DONE verdict=REPRO|CLEAN max_ms=<n> (REPRO iff any observed
 *   latency class >= 1000 ms)
 *
 * Build: riscv64-linux-gnu-gcc -static -O2 -pthread
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <pthread.h>

/* futex op constants (older glibc headers lack them) */
#ifndef FUTEX_WAIT
#define FUTEX_WAIT 0
#define FUTEX_WAKE 1
#endif
#ifndef FUTEX_PRIVATE_FLAG
#define FUTEX_PRIVATE_FLAG 128
#endif

/* ---- futex (musl/glibc may lack SYS_futex wrappers) ---- */
static long fx(volatile uint32_t *uaddr, int op, uint32_t val,
               const struct timespec *to, uint32_t *uaddr2, uint32_t val3)
{
    return syscall(SYS_futex, (long *)uaddr, op, val, to, uaddr2, val3);
}
#define FX_WAIT(u, v)  fx((u), FUTEX_WAIT | FUTEX_PRIVATE_FLAG, (v), 0, 0, 0)
#define FX_WAKE(u, n)  fx((u), FUTEX_WAKE | FUTEX_PRIVATE_FLAG, (n), 0, 0, 0)

static uint64_t now_us(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000ull + ts.tv_nsec / 1000;
}

static long gettid_(void) { return syscall(SYS_gettid); }

/* ---- latency sample buffers ---- */
#define MAXSAMP (1u << 20)
static uint64_t *samples_wake, *samples_pong, *samples_pipe, *samples_wd;
static uint32_t n_wake, n_pong_s, n_pipe_s, n_wd;
static volatile int stop_all;

static void add_sample(uint64_t **buf, uint32_t *n, uint64_t v)
{
    if (*n < MAXSAMP)
        (*buf)[(*n)++] = v;
}

static int cmp_u64(const void *a, const void *b)
{
    uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
    return x < y ? -1 : x > y ? 1 : 0;
}

static void report(const char *cls, uint64_t *buf, uint32_t n, uint64_t *max_out)
{
    if (n == 0) {
        printf("SREPRO %s n=0\n", cls);
        return;
    }
    qsort(buf, n, sizeof(uint64_t), cmp_u64);
    uint64_t p50 = buf[n / 2], p99 = buf[(n * 99) / 100], mx = buf[n - 1];
    uint32_t gt250 = 0, gt1000 = 0;
    for (uint32_t i = 0; i < n; i++) {
        if (buf[i] >= 250000) gt250++;
        if (buf[i] >= 1000000) gt1000++;
    }
    printf("SREPRO %s n=%u p50=%lluus p99=%lluus max=%lluus gt250ms=%u gt1s=%u\n",
           cls, n, (unsigned long long)p50, (unsigned long long)p99,
           (unsigned long long)mx, gt250, gt1000);
    if (max_out && mx > *max_out) *max_out = mx;
}

/* ---- wake storm ---- */
#define MAXW 64
struct wslot {
    volatile uint64_t seq;   /* futex word lives in the low 32 bits */
    volatile uint64_t tstamp;
    char pad[64 - 16];
};
static struct wslot wslots[MAXW];
static int n_workers = 8, n_wakers = 2;

static void *waker_fn(void *arg)
{
    long id = (long)arg;
    unsigned rr = (unsigned)id;
    printf("SREPRO tid %ld role=waker%d\n", gettid_(), (int)id);
    while (!stop_all) {
        for (int k = 0; k < n_workers; k++) {
            struct wslot *s = &wslots[(rr + k) & (MAXW - 1)];
            uint64_t seq = s->seq + 1;
            s->tstamp = now_us();
            __sync_synchronize();
            s->seq = seq;
            FX_WAKE((volatile uint32_t *)&s->seq, 1);
            rr++;
        }
        /* pace: one burst per ~2 ms so wakeups stay the dominant event */
        struct timespec ts = {0, 2 * 1000 * 1000};
        clock_nanosleep(CLOCK_MONOTONIC, 0, &ts, 0);
    }
    return 0;
}

static void *worker_fn(void *arg)
{
    long id = (long)arg;
    struct wslot *s = &wslots[id];
    printf("SREPRO tid %ld role=worker%ld\n", gettid_(), id);
    while (!stop_all) {
        uint64_t seq = s->seq;
        long r = FX_WAIT((volatile uint32_t *)&s->seq, (uint32_t)seq);
        (void)r;
        __sync_synchronize();
        if (s->seq != seq) {
            uint64_t d = now_us() - s->tstamp;
            add_sample(&samples_wake, &n_wake, d);
            if (d >= 250000)
                printf("SREPRO wake-lat %lluus tid %ld\n",
                       (unsigned long long)d, gettid_());
        }
    }
    return 0;
}

/* ---- futex pong ---- */
struct pslot {
    volatile uint64_t seq; /* hop counter; low 32 bits = futex word */
    volatile uint64_t tstamp;
    char pad[64 - 16];
};
static struct pslot pslots[MAXW];
static int n_pong = 4;

static void *pong_fn(void *arg)
{
    long id = (long)arg;         /* pair id; thread parity from stack? no: */
    return 0;                    /* replaced by pong_pair below */
}
struct pong_arg { int pair; int side; };
static void *pong_pair(void *arg)
{
    struct pong_arg *a = arg;
    struct pslot *s = &pslots[a->pair];
    printf("SREPRO tid %ld role=pong%d.%d\n", gettid_(), a->pair, a->side);
    /* Turn protocol: turn even = side 0 moves, odd = side 1 moves. The
     * mover flips the turn and wakes; the waiter sleeps only while the
     * turn is against it. No lost-wake window (each WAIT re-reads under
     * the loop). Side 0 measures the full round trip (its move + peer's
     * reply move). */
    while (!stop_all) {
        uint64_t t = s->seq;
        if (a->side == 0) {
            while ((t & 1) && !stop_all) {
                FX_WAIT((volatile uint32_t *)&s->seq, (uint32_t)t);
                t = s->seq;
            }
            if (stop_all)
                break;
            s->tstamp = now_us();
            __sync_synchronize();
            s->seq = t + 1;
            FX_WAKE((volatile uint32_t *)&s->seq, 1);
            t = s->seq;
            while ((t & 1) && !stop_all) {
                FX_WAIT((volatile uint32_t *)&s->seq, (uint32_t)t);
                t = s->seq;
            }
            if (!stop_all && !(t & 1)) {
                uint64_t d = now_us() - s->tstamp;
                add_sample(&samples_pong, &n_pong_s, d);
                if (d >= 250000)
                    printf("SREPRO pong-lat %lluus pair %d\n",
                           (unsigned long long)d, a->pair);
            }
        } else {
            while (!(t & 1) && !stop_all) {
                FX_WAIT((volatile uint32_t *)&s->seq, (uint32_t)t);
                t = s->seq;
            }
            if (stop_all)
                break;
            __sync_synchronize();
            s->seq = t + 1;
            FX_WAKE((volatile uint32_t *)&s->seq, 1);
        }
    }
    return 0;
}

/* ---- cpu hogs ---- */
static int n_hogs = 2;
static void *hog_fn(void *arg)
{
    long id = (long)arg;
    volatile uint64_t acc = 1;
    printf("SREPRO tid %ld role=hog%ld\n", gettid_(), id);
    while (!stop_all) {
        for (int i = 0; i < 1000; i++)
            acc = acc * 6364136223846793005ull + 1442695040888963407ull;
    }
    (void)acc;
    return 0;
}

/* ---- fork churn ---- */
static int n_churn = 2;
static void *churn_fn(void *arg)
{
    long id = (long)arg;
    printf("SREPRO tid %ld role=churn%ld\n", gettid_(), id);
    /* fork from a thread is avoided: dedicated loop with vfork-free plain
     * fork; child _exits immediately. Rate: unbounded. */
    while (!stop_all) {
        pid_t p = fork();
        if (p == 0)
            _exit(0);
        if (p > 0)
            waitpid(p, 0, 0);
    }
    return 0;
}

/* ---- pipe req/rep ---- */
static int n_pipe = 4;
struct rring { int c2s[2], s2c[2]; };
static struct rring rrings[MAXW];
static void *pipe_client(void *arg)
{
    long id = (long)arg;
    struct rring *r = &rrings[id];
    char b = 1;
    printf("SREPRO tid %ld role=pipe_c%ld\n", gettid_(), id);
    while (!stop_all) {
        uint64_t t0 = now_us();
        if (write(r->c2s[1], &b, 1) != 1)
            break;
        char rb;
        if (read(r->s2c[0], &rb, 1) != 1)
            break;
        uint64_t d = now_us() - t0;
        add_sample(&samples_pipe, &n_pipe_s, d);
        if (d >= 250000)
            printf("SREPRO pipe-rt %lluus pair %ld\n",
                   (unsigned long long)d, id);
    }
    return 0;
}
static void *pipe_server(void *arg)
{
    long id = (long)arg;
    struct rring *r = &rrings[id];
    char b;
    printf("SREPRO tid %ld role=pipe_s%ld\n", gettid_(), id);
    while (!stop_all) {
        if (read(r->c2s[0], &b, 1) != 1)
            break;
        if (write(r->s2c[1], &b, 1) != 1)
            break;
    }
    return 0;
}

/* ---- watchdog ---- */
static void *wd_fn(void *arg)
{
    (void)arg;
    printf("SREPRO tid %ld role=wd\n", gettid_());
    while (!stop_all) {
        uint64_t t0 = now_us();
        struct timespec ts = {0, 50 * 1000 * 1000};
        clock_nanosleep(CLOCK_MONOTONIC, 0, &ts, 0);
        uint64_t d = now_us() - t0;
        add_sample(&samples_wd, &n_wd, d);
        if (d >= 250000)
            printf("SREPRO wd-over %lluus at %lluus tid %ld\n",
                   (unsigned long long)d, (unsigned long long)t0, gettid_());
    }
    return 0;
}

/* ---- config ---- */
static int secs = 60;
static void parse_cfg(const char *s)
{
    char buf[512];
    snprintf(buf, sizeof(buf), "%s", s);
    char *save = 0, *tok = strtok_r(buf, ":", &save);
    while (tok) {
        int v; char *comma;
        switch (tok[0]) {
        case 'W':
            v = atoi(tok + 1);
            if (v > 0) n_workers = v < MAXW ? v : MAXW;
            if ((comma = strchr(tok, ',')) && comma[1] == 'x') {
                v = atoi(comma + 2);
                if (v > 0) n_wakers = v < 8 ? v : 8;
            }
            break;
        case 'P':
            v = atoi(tok + 1);
            if (v > 0) n_pong = v < MAXW ? v : MAXW;
            break;
        case 'H':
            v = atoi(tok + 1);
            if (v >= 0) n_hogs = v < 16 ? v : 16;
            break;
        case 'F':
            v = atoi(tok + 1);
            if (v >= 0) n_churn = v < 8 ? v : 8;
            break;
        case 'R':
            v = atoi(tok + 1);
            if (v >= 0) n_pipe = v < MAXW ? v : MAXW;
            break;
        case 'T':
            v = atoi(tok + 1);
            if (v > 0) secs = v;
            break;
        }
        tok = strtok_r(0, ":", &save);
    }
}

static void read_cmdline_cfg(void)
{
    /* /proc is not mounted when we run as init on the tiny probe rootfs */
    mkdir("/proc", 0755);
    mount("proc", "/proc", "proc", 0, 0);
    int fd = open("/proc/cmdline", O_RDONLY);
    if (fd < 0)
        return;
    char buf[2048] = {0};
    int n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0)
        return;
    char *p = strstr(buf, "schedrepro=");
    if (!p)
        return;
    p += strlen("schedrepro=");
    char *e = p;
    while (*e && *e != ' ' && *e != '\n')
        e++;
    char cfg[256];
    int len = e - p < 255 ? e - p : 255;
    memcpy(cfg, p, len);
    cfg[len] = 0;
    parse_cfg(cfg);
}

int main(int argc, char **argv)
{
    static uint64_t sbuf[4][MAXSAMP];
    samples_wake = sbuf[0]; samples_pong = sbuf[1];
    samples_pipe = sbuf[2]; samples_wd = sbuf[3];

    parse_cfg("W8,x2:P4:H2:F2:R4:T60");
    read_cmdline_cfg();
    if (argc > 1 && argv[1][0])
        parse_cfg(argv[1]);

    printf("SREPRO start cfg=W%d,x%d:P%d:H%d:F%d:R%d:T%d pid=%d\n",
           n_workers, n_wakers, n_pong, n_hogs, n_churn, n_pipe, secs,
           (int)getpid());
    fflush(stdout);

    for (int i = 0; i < n_pipe; i++)
        pipe(rrings[i].c2s), pipe(rrings[i].s2c);

    pthread_t th[256];
    int nt = 0;
    for (long i = 0; i < n_wakers; i++)
        pthread_create(&th[nt++], 0, waker_fn, (void *)i);
    for (long i = 0; i < n_workers; i++)
        pthread_create(&th[nt++], 0, worker_fn, (void *)i);
    for (int i = 0; i < n_pong; i++) {
        static struct pong_arg pa[MAXW];
        pa[i].pair = i; pa[i].side = 0;
        pthread_create(&th[nt++], 0, pong_pair, &pa[i]);
        static struct pong_arg pc[MAXW];
        pc[i].pair = i; pc[i].side = 1;
        pthread_create(&th[nt++], 0, pong_pair, &pc[i]);
    }
    for (long i = 0; i < n_hogs; i++)
        pthread_create(&th[nt++], 0, hog_fn, (void *)i);
    for (long i = 0; i < n_churn; i++)
        pthread_create(&th[nt++], 0, churn_fn, (void *)i);
    for (long i = 0; i < n_pipe; i++) {
        pthread_create(&th[nt++], 0, pipe_client, (void *)i);
        pthread_create(&th[nt++], 0, pipe_server, (void *)i);
    }
    pthread_create(&th[nt++], 0, wd_fn, 0);

    struct timespec ts = {secs, 0};
    nanosleep(&ts, 0);
    stop_all = 1;
    /* wake everyone so they can exit */
    for (int i = 0; i < MAXW; i++) {
        FX_WAKE((volatile uint32_t *)&wslots[i].seq, 1);
        FX_WAKE((volatile uint32_t *)&pslots[i].seq, 1);
    }
    for (int i = 0; i < n_pipe; i++) {
        char b = 1;
        write(rrings[i].c2s[1], &b, 1);
    }
    /* Do NOT join: pipe clients can sit in a blocking read whose server
     * already exited, and a hung join would eat the verdict. Give the
     * reporters half a second to land their last samples, then report
     * from the shared buffers and power off. */
    struct timespec g = {0, 500 * 1000 * 1000};
    nanosleep(&g, 0);

    uint64_t mx = 0;
    report("wake", samples_wake, n_wake, &mx);
    report("pong", samples_pong, n_pong_s, &mx);
    report("pipe", samples_pipe, n_pipe_s, &mx);
    report("wd", samples_wd, n_wd, &mx);
    printf("SREPRO DONE verdict=%s max_ms=%llu\n",
           mx >= 1000000ull ? "REPRO" : "CLEAN",
           (unsigned long long)(mx / 1000));
    fflush(stdout);
    /* as init: power off */
    syscall(SYS_reboot, 0xfee1dead, 672274793, 0x4321fedc);
    _exit(0);
}
