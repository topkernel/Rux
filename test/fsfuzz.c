/* fsfuzz.c — stress-fleet random-syscall fuzzer (PID 1, never exits).
 *
 * Calls a curated set of syscall entry points with randomized argument
 * tuples drawn from interesting pools:
 *   - fd pool: {-2,-1,0,1,2,3,100}  (valid-looking and invalid fds)
 *   - addr pool: {0, 1, 0x1000, stack±, map-low, map-high, -1UL, 0xdead0000}
 *   - len pool: {0,1,4096,65536,1<<20, -1, -4096}
 *   - flag pool: {0,1,0x7fff, O_RDONLY|O_CREAT, MAP_*, PROT_* mixes}
 * plus a few orchestrated mini-sequences (open-write-close-unlink,
 * pipe-write-read-close, socket-create-bind-close) so real resource
 * lifetimes get churned, not just rejected calls.
 *
 * The kernel must answer every tuple with a normal error code — any
 * panic, oops, watchdog trip, or wedged serial is the catch. Expected
 * errors are NOT printed (too noisy); only sequence failures that are
 * unexpected (e.g. pipe() failing with EMFILE at low fd counts) print
 * FSFUZZ-ODD lines for the sweep to classify.
 *
 * Build: riscv64-musl-gcc -static -O1 -o fsfuzz fsfuzz.c
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static int saved_out = -1; /* probe stdout independence (s10 self-park
 * fix): the raw pool closes 0/1/2; every marker print goes through the
 * saved descriptor so the fuzzer can never blind its own trace. */
static unsigned long seed = 7;
static inline unsigned long rnd(void) {
    seed = seed * 6364136223846793005UL + 1442695040888963407UL;
    return seed >> 13;
}
#define PICK(arr) ((arr)[rnd() % (sizeof(arr) / sizeof((arr)[0]))])

static const int fd_pool[] = {-2, -1, 0, 1, 2, 3, 7, 100, 1023};
static const long len_pool[] = {0, 1, 4095, 4096, 65536, 1 << 20, -1, -4096, 0x7fffffff};
static const int flag_pool[] = {0, 1, 0x7fff, 0x8000, O_RDONLY, O_RDWR | O_CREAT,
                                O_WRONLY | O_TRUNC | O_CREAT, 0644};
static const long off_pool[] = {0, 1, -1, -4096, 1 << 30, 1L << 40, -1L << 40};

static uintptr_t addr_pool[10];
static unsigned addr_init;

static void init_addrs(void) {
    addr_pool[0] = 0;
    addr_pool[1] = 1;
    addr_pool[2] = 0x1000;
    addr_pool[3] = 0xdead0000UL;          /* unmapped-but-plausible */
    addr_pool[4] = (uintptr_t)&errno;      /* valid own data */
    addr_pool[5] = (uintptr_t)init_addrs;  /* valid own text (RO!) */
    addr_pool[6] = (uintptr_t)&seed - 64;  /* near-stack/data */
    addr_pool[7] = (uintptr_t)0x7ffffffff000UL; /* top of user range */
    addr_pool[8] = (uintptr_t)-4096;       /* kernel-adjacent */
    addr_pool[9] = (uintptr_t)0xfffffffffffff000UL;
    addr_init = 1;
}

static void raw_syscalls(unsigned n) {
    for (unsigned i = 0; i < n; i++) {
        long nr = (long)(rnd() % 60) + 1; /* low syscall numbers only */
        long a = (long)PICK(fd_pool);
        long b = (long)PICK(len_pool);
        long c = (long)PICK(flag_pool);
        long d = (long)PICK(off_pool);
        long e = (long)PICK(addr_pool);
        long f = (long)(rnd() & 0xffff);
        /* s10 self-park fix: the stream once built an empty epoll on a
         * closed-then-reused fd 0 and parked in epoll_pwait forever on
         * an unbounded timeout draw (Linux would block too — but a
         * parked fuzzer fuzzes nothing). Clamp every blocking-wait
         * timeout in the low-nr pool to a bounded probe-friendly value;
         * all other arguments stay fully chaotic. */
        if (nr == 22 && (d < 0 || d > 200))
            d = 200;
        (void)syscall((long)nr, a, b, c, d, e, f);
    }
}

static void seq_file(void) {
    char path[64];
    snprintf(path, sizeof(path), "/tmp/fz%lu", rnd() % 100000);
    int fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        if (errno != ENOSPC && errno != ENOMEM)
            dprintf(saved_out, "FSFUZZ-ODD open %s errno=%d\n", path, errno);
        return;
    }
    char buf[512];
    memset(buf, 0x61, sizeof(buf));
    if (write(fd, buf, sizeof(buf)) != (ssize_t)sizeof(buf))
        dprintf(saved_out, "FSFUZZ-ODD short-write errno=%d\n", errno);
    if (lseek(fd, (off_t)PICK(off_pool), SEEK_SET) < 0 && errno != EINVAL)
        dprintf(saved_out, "FSFUZZ-ODD lseek errno=%d\n", errno);
    (void)read(fd, buf, (size_t)PICK(len_pool));
    /* rename/unlink churn on the same namespace */
    char path2[64];
    snprintf(path2, sizeof(path2), "/tmp/fz%lu", rnd() % 100000);
    if (rename(path, path2) < 0 && errno != ENOENT && errno != ENOSPC)
        dprintf(saved_out, "FSFUZZ-ODD rename errno=%d\n", errno);
    unlink(path);
    unlink(path2);
    close(fd);
}

static void seq_pipe(void) {
    int pfd[2];
    if (pipe(pfd) < 0) {
        dprintf(saved_out, "FSFUZZ-ODD pipe errno=%d\n", errno);
        return;
    }
    char buf[256];
    memset(buf, 0x62, sizeof(buf));
    (void)write(pfd[1], buf, sizeof(buf));
    (void)read(pfd[0], buf, (size_t)PICK(len_pool));
    close(pfd[0]);
    close(pfd[1]);
}

static void seq_socket(void) {
    int s = socket(AF_UNIX, SOCK_STREAM, 0);
    if (s < 0) {
        dprintf(saved_out, "FSFUZZ-ODD socket errno=%d\n", errno);
        return;
    }
    struct sockaddr_un_storage { unsigned char x[128]; } addr;
    memset(&addr, 0, sizeof(addr));
    (void)bind(s, (void *)&addr, (socklen_t)PICK(len_pool));
    (void)listen(s, (int)(rnd() % 16));
    close(s);
}

static void seq_madvise(void) {
    if (!addr_init) return;
    uintptr_t a = PICK(addr_pool);
    (void)madvise((void *)a, (size_t)PICK(len_pool), (int)(rnd() % 16));
}

#ifndef SYS_perf_event_open
#define SYS_perf_event_open 241 /* asm-generic (riscv64) */
#endif

static unsigned long perf_ok; /* census: valid-shape opens that succeeded */

/* perf_event_open(2) — the fresh D-7-set syscall surface on main
 * 0f806d8b: PERF_TYPE_SOFTWARE cpu/task-clock sampling with an attr
 * head whose copy-in is gated by the user's own size field (VER0=64,
 * kernel accepts 64..1024). Contract: every tuple answers a normal
 * errno or hands back a usable fd — panic, watchdog, or a wedged
 * serial is the catch. Each call mixes chaos draws (full-garbage blob,
 * targeted size/type/config/flags draws, sometimes a garbage attr
 * pointer from addr_pool for the EFAULT class) with one valid-shape
 * draw so the event lifecycle (open, ARMED count up, read() record
 * drain, close, ARMED down) actually churns alongside the rejects. */
static void seq_perf(void) {
    static const uint32_t size_pool[] = {0, 1, 8, 63, 64, 65, 72, 128,
                                         256, 1024, 1032, 0x7fffffff, 0xffffffffu};
    static const uint32_t type_pool[] = {0, 1, 2, 3, 4, 5, 6, 13, 0xffffffffu};
    static const uint64_t cfg_pool[] = {0, 1, 2, 8, 0x7fff, 0xffffffffffffffffUL};
    static const uint64_t per_pool[] = {0, 1, 16, 1000, 0x100000, 0xffffffffffffffffUL};
    static const long pid_pool[] = {-2, -1, 0, 1, 2, 100, 0x7fffffff};
    static const long cpu_pool[] = {-2, -1, 0, 1, 2, 3, 64, 0x7fffffff};
    static const long pflags_pool[] = {0, 1, 2, 3, 4, 8, 9, 0x10, 0xffff, 0x7fffffffffffL};

    unsigned char blob[128];
    unsigned long w;
    for (unsigned i = 0; i < sizeof(blob) / 8; i++) {
        w = rnd();
        memcpy(blob + i * 8, &w, 8);
    }
    uint32_t *p_type = (uint32_t *)(blob + 0);
    uint32_t *p_size = (uint32_t *)(blob + 4);
    uint64_t *p_cfg = (uint64_t *)(blob + 8);
    uint64_t *p_per = (uint64_t *)(blob + 16);
    uint64_t *p_st = (uint64_t *)(blob + 24);
    uint64_t *p_rf = (uint64_t *)(blob + 32);
    uint64_t *p_fl = (uint64_t *)(blob + 40);

    int fds[4], nf = 0;
    for (int k = 0; k < 3; k++) {
        *p_type = PICK(type_pool);
        *p_size = PICK(size_pool);
        *p_cfg = PICK(cfg_pool);
        *p_per = PICK(per_pool);
        *p_st = rnd();
        *p_rf = rnd() & 0xf;
        *p_fl = rnd();
        void *ap = (rnd() & 7) == 0 ? (void *)PICK(addr_pool) : (void *)blob;
        long fd = syscall(SYS_perf_event_open, ap, PICK(pid_pool),
                          PICK(cpu_pool), (long)PICK(fd_pool), PICK(pflags_pool));
        if (fd >= 0 && nf < 4) fds[nf++] = (int)fd;
    }

    /* valid-shape draw: SOFTWARE cpu/task-clock, VER0 size, IP-only */
    memset(blob, 0, 64);
    *p_type = 1;             /* PERF_TYPE_SOFTWARE */
    *p_size = 64;            /* PERF_ATTR_SIZE_VER0 */
    *p_cfg = (rnd() & 1);    /* PERF_COUNT_SW_CPU_CLOCK / _TASK_CLOCK */
    *p_per = 1 + (rnd() & 0xffff); /* nonzero sample period */
    *p_st = 1;               /* PERF_SAMPLE_IP */
    int percpu = rnd() & 1;
    long vfd = syscall(SYS_perf_event_open, blob,
                       percpu ? -1L : 0L, percpu ? 0L : -1L, -1L,
                       (rnd() & 1) ? 0L : 8L /* [PERF_FLAG_FD_CLOEXEC] */);
    if (vfd >= 0) {
        fds[nf++] = (int)vfd;
        perf_ok++;
        if ((perf_ok & 0xff) == 0)
            dprintf(saved_out, "FSFUZZ-PERF ok=%lu\n", perf_ok);
    } else if (errno != EINVAL && errno != EFAULT && errno != ENOSYS &&
               errno != EACCES && errno != ESRCH) {
        dprintf(saved_out, "FSFUZZ-ODD perf errno=%d\n", errno);
    }

    /* lifecycle churn: read() the sample-record drain, then close */
    char rbuf[512];
    for (int k = 0; k < nf; k++) {
        (void)read(fds[k], rbuf, (size_t)PICK(len_pool));
        close(fds[k]);
    }
}

int main(int argc, char **argv) {
    seed = (argc > 1) ? (unsigned long)atoll(argv[1]) : 7;
    setvbuf(stdout, NULL, _IOLBF, 0);
    /* saved fd MUST sit outside the fuzz fd pool {-2,-1,0,1,2,3,7,100,
     * 1023}: the raw stream closes pool fds, and a saved fd inside the
     * pool gets closed too (the fd-3 dup variant wedged exactly like
     * the original fd-1 blindness). F_DUPFD from 900 is pool-safe. */
    saved_out = fcntl(1, 1030 /* F_DUPFD */, 900);
    if (saved_out < 0) saved_out = dup(1);
    init_addrs();
    dprintf(saved_out, "FSFUZZ-START seed=%lu pid=%d\n", seed, getpid());
    for (unsigned long i = 0;; i++) {
        raw_syscalls(200);
        switch (rnd() % 6) {
        case 0: seq_file(); break;
        case 1: seq_pipe(); break;
        case 2: seq_socket(); break;
        case 3: seq_madvise(); break;
        case 4: {
            /* tiny fork child that immediately raw-syscalls and exits */
            pid_t pid = fork();
            if (pid == 0) { raw_syscalls(50); _exit(0); }
            if (pid > 0) waitpid(pid, NULL, 0);
            break;
        }
        case 5: seq_perf(); break;
        }
        if ((i & 0x3f) == 0) {
            dprintf(saved_out, "FSFUZZ-BEAT %lu\n", i);
            usleep(20000);
        }
    }
    return 0; /* unreachable */
}
