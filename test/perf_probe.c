// perf_probe.c — userspace smoke for the perf_event_open software-clock
// slice (PERF_TYPE_SOFTWARE cpu-clock/task-clock, sample_period,
// PERF_SAMPLE_IP records via read(2)).
//
// Checks (each prints PERF_PROBE <name>: PASS/FAIL):
//   open-task     task-clock on self (pid=0) opens
//   user-sample   after a 600 ms user spin, poll says POLLIN and
//                 read() returns 16-byte PERF_RECORD_SAMPLE records
//                 with misc=USER and a non-zero ip
//   cpu-sample    per-CPU cpu-clock (pid=-1, cpu=0) yields records
//   excl-user     exclude_user event yields only KERNEL-misc records
//                 (count may be 0; any record must decode as KERNEL)
//   lost/short    a 4-byte read is EINVAL (record does not fit)
//   err-matrix    EINVAL: hardware type, extra sample bits, freq mode,
//                 inherit, read_format!=0, period=0, both pid&cpu -1,
//                 group_fd>=0, cpu out of range; ESRCH: dead pid;
//                 EFAULT: bad attr pointer
//   close-reuse   close(2) then a fresh open works (registry clean)
//
// Build: musl-static via test/perf-smoke.sh (rootfs libc — the distro
// glibc carries RVV the standard QEMU cpu lacks).

#include <stdint.h>
#include <stddef.h>
#include <errno.h>
#include <unistd.h>
#include <fcntl.h>
#include <poll.h>
#include <time.h>
#include <sys/syscall.h>

#define PERF_TYPE_HARDWARE 0
#define PERF_TYPE_SOFTWARE 1
#define PERF_COUNT_SW_CPU_CLOCK 0
#define PERF_COUNT_SW_TASK_CLOCK 1
#define PERF_SAMPLE_IP (1ULL << 0)
#define PERF_RECORD_LOST 2
#define PERF_RECORD_SAMPLE 9
#define MISC_KERNEL 2
#define MISC_USER 1

// The ABI VER0 (64-byte) attr head.
struct pattr {
    uint32_t type;
    uint32_t size;
    uint64_t config;
    uint64_t sample_period;
    uint64_t sample_type;
    uint64_t read_format;
    uint64_t flags1;
    uint32_t wakeup_events;
    uint32_t pad0;
    uint64_t pad1;
};

static long pe_open(struct pattr *a, int32_t pid, int32_t cpu,
                    int32_t group_fd, uint64_t flags)
{
    return syscall(241 /* __NR_perf_event_open, asm-generic */,
                   a, pid, cpu, group_fd, flags);
}

static int fails;

static void check(const char *name, int ok)
{
    if (ok) {
        __builtin_printf("PERF_PROBE %s: PASS\n", name);
    } else {
        __builtin_printf("PERF_PROBE %s: FAIL\n", name);
        fails++;
    }
}

static uint64_t now_ms(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000u + ts.tv_nsec / 1000000u;
}

static void spin_user_ms(uint64_t ms)
{
    volatile uint64_t sink = 0;
    uint64_t t0 = now_ms();
    while (now_ms() - t0 < ms) {
        for (int i = 0; i < 100000; i++)
            sink += i;
    }
    (void)sink;
}

/* User spin interleaved with kernel work (a /proc read per round): the
 * timer tick then lands in KERNEL mode regularly — what the callchain
 * section needs sampled. */
static void spin_mixed_ms(uint64_t ms)
{
    volatile uint64_t sink = 0;
    uint8_t scratch[512];
    uint64_t t0 = now_ms();
    while (now_ms() - t0 < ms) {
        for (int i = 0; i < 20000; i++)
            sink += i;
        int fd = open("/proc/1/status", O_RDONLY);
        if (fd >= 0) {
            while (read(fd, scratch, sizeof(scratch)) > 0) {
            }
            close(fd);
        }
    }
    (void)sink;
}

struct rec {
    uint32_t type;
    uint16_t misc;
    uint16_t size;
    uint64_t ip;
};

// Decode up to `max` records from buf; returns count, sets *n_user /
// *n_kernel / *bad. Tracks LOST records too.
static int decode(const uint8_t *buf, size_t len, int max, int *n_user,
                  int *n_kernel, int *n_lost, int *bad)
{
    int n = 0;
    *n_user = *n_kernel = *n_lost = *bad = 0;
    size_t off = 0;
    while (off + 8 <= len && n < max) {
        struct rec r;
        __builtin_memcpy(&r, buf + off, 8);
        if (r.size < 8 || off + r.size > len) {
            (*bad)++;
            break;
        }
        if (r.type == PERF_RECORD_SAMPLE && r.size == 16) {
            __builtin_memcpy(&r.ip, buf + off + 8, 8);
            if (r.misc == MISC_USER)
                (*n_user)++;
            else if (r.misc == MISC_KERNEL)
                (*n_kernel)++;
            else
                (*bad)++;
            __builtin_printf("  sample misc=%u ip=0x%llx\n", r.misc,
                             (unsigned long long)r.ip);
        } else if (r.type == PERF_RECORD_LOST && r.size == 24) {
            (*n_lost)++;
        } else {
            (*bad)++;
        }
        off += r.size;
        n++;
    }
    return n;
}

int main(void)
{
    // ---- 1. task-clock on self, IP-only samples, 20 ms period ----
    struct pattr a;
    __builtin_memset(&a, 0, sizeof(a));
    a.type = PERF_TYPE_SOFTWARE;
    a.size = 64;
    a.config = PERF_COUNT_SW_TASK_CLOCK;
    a.sample_period = 20000000u; /* 20 ms = 2 ticks */
    a.sample_type = PERF_SAMPLE_IP;

    long fd = pe_open(&a, 0, -1, -1, 0);
    check("open-task", fd >= 0);
    if (fd < 0) {
        __builtin_printf("PERF_PROBE RESULT: FAIL (open errno=%d)\n", errno);
        return 1;
    }

    spin_user_ms(600);

    struct pollfd pfd = { .fd = (int)fd, .events = 1 /* POLLIN */ };
    int pr = poll(&pfd, 1, 3000);
    check("pollin", pr == 1 && (pfd.revents & 1));

    uint8_t buf[4096];
    long n = read((int)fd, buf, sizeof(buf));
    int n_user = 0, n_kernel = 0, n_lost = 0, bad = 0;
    int nrec = 0;
    if (n > 0)
        nrec = decode(buf, (size_t)n, 64, &n_user, &n_kernel, &n_lost, &bad);
    __builtin_printf("  read=%ld recs=%d user=%d kern=%d lost=%d bad=%d\n",
                     n, nrec, n_user, n_kernel, n_lost, bad);
    check("user-sample",
          n > 0 && nrec > 0 && n_user > 0 && bad == 0 && (n % 16) == 0);

    // ---- 2. short read is EINVAL ----
    int8_t tiny[4];
    check("short-einval", read((int)fd, tiny, 4) == -1 && errno == 22);
    close((int)fd);

    // ---- 3. per-CPU cpu-clock ----
    a.config = PERF_COUNT_SW_CPU_CLOCK;
    fd = pe_open(&a, -1, 0, -1, 0);
    check("open-cpu", fd >= 0);
    if (fd >= 0) {
        spin_user_ms(400);
        struct pollfd pfd2 = { .fd = (int)fd, .events = 1 };
        pr = poll(&pfd2, 1, 3000);
        n = read((int)fd, buf, sizeof(buf));
        nrec = decode(buf, n > 0 ? (size_t)n : 0, 64, &n_user, &n_kernel,
                      &n_lost, &bad);
        __builtin_printf("  cpu read=%ld recs=%d\n", n, nrec);
        check("cpu-sample", pr >= 0 && n > 0 && nrec > 0 && bad == 0);
        close((int)fd);
    }

    // ---- 4. exclude_user (kernel-only records must be KERNEL-misc) ----
    a.config = PERF_COUNT_SW_TASK_CLOCK;
    a.flags1 = 1u << 4; /* exclude_user */
    fd = pe_open(&a, 0, -1, -1, 0);
    check("open-excl", fd >= 0);
    if (fd >= 0) {
        spin_user_ms(400);
        struct pollfd pfd3 = { .fd = (int)fd, .events = 1 };
        pr = poll(&pfd3, 1, 1000);
        if (pr == 1 && (pfd3.revents & 1)) {
            n = read((int)fd, buf, sizeof(buf));
            nrec = decode(buf, n > 0 ? (size_t)n : 0, 64, &n_user, &n_kernel,
                          &n_lost, &bad);
        } else {
            /* No kernel-mode tick landed in the window: legal (the fd
             * is blocking; reading would park forever). Zero-record
             * observation, not a failure of the filter. Reset the
             * decode accumulators — they still hold the previous
             * section's counts. */
            n = 0;
            nrec = 0;
            n_user = n_kernel = n_lost = bad = 0;
        }
        __builtin_printf("  excl poll=%d read=%ld recs=%d kern=%d\n", pr, n,
                         nrec, n_kernel);
        check("excl-user", n >= 0 && n_user == 0 && bad == 0);
        close((int)fd);
    }

    // ---- 5. callchain samples (PERF_SAMPLE_CALLCHAIN) ----
    // Two shapes must come back:
    //   a) per-CPU cpu-clock: kernel-mode samples carry nr = 1 +
    //      frames, ips[0] == PERF_CONTEXT_KERNEL (0xffffffffffffff00),
    //      ips[1..] kernel text return addresses (the wedge-hunt food);
    //   b) task-clock on self while spinning in USER mode: records are
    //      24 bytes with nr = 0 (no user-stack walker — honest empty).
    {
        struct pattr c;
        __builtin_memset(&c, 0, sizeof(c));
        c.type = PERF_TYPE_SOFTWARE;
        c.size = 64;
        c.config = PERF_COUNT_SW_CPU_CLOCK;
        c.sample_period = 20000000u;
        c.sample_type = PERF_SAMPLE_IP | (1ULL << 5); /* CALLCHAIN */
        c.flags1 = 1u << 4; /* exclude_user: kernel-mode samples only */
        long cfd = pe_open(&c, -1, 0, -1, 0);
        check("open-chain", cfd >= 0);
        if (cfd >= 0) {
            spin_mixed_ms(1200);
            struct pollfd pfdc = { .fd = (int)cfd, .events = 1 };
            poll(&pfdc, 1, 3000);
            n = read((int)cfd, buf, sizeof(buf));
            int chains = 0, kframes = 0, badc = 0;
            size_t off = 0;
            while (n > 0 && off + 8 <= (size_t)n) {
                struct rec r;
                __builtin_memcpy(&r, buf + off, 8);
                if (r.size < 8 || off + r.size > (size_t)n) {
                    badc++;
                    break;
                }
                if (r.type == PERF_RECORD_SAMPLE && r.size >= 24) {
                    uint64_t nr;
                    __builtin_memcpy(&nr, buf + off + 16, 8);
                    if (r.misc == MISC_KERNEL && nr >= 2) {
                        uint64_t m0, f1;
                        __builtin_memcpy(&m0, buf + off + 24, 8);
                        __builtin_memcpy(&f1, buf + off + 32, 8);
                        if (m0 == 0xffffffffffffff00ULL && f1 != 0) {
                            chains++;
                            kframes += (int)nr - 1;
                            __builtin_printf("  chain nr=%llu f1=0x%llx\n",
                                             (unsigned long long)nr,
                                             (unsigned long long)f1);
                        } else {
                            badc++;
                        }
                    }
                }
                off += r.size;
            }
            __builtin_printf("  chain read=%ld chains=%d frames=%d bad=%d\n",
                             n, chains, kframes, badc);
            check("chain-kernel", chains > 0 && kframes > 0 && badc == 0);
            close((int)cfd);
        }

        // user-mode chain records: nr == 0, size 24.
        c.config = PERF_COUNT_SW_TASK_CLOCK;
        c.flags1 = 0;
        long ufd = pe_open(&c, 0, -1, -1, 0);
        check("open-chain-user", ufd >= 0);
        if (ufd >= 0) {
            spin_user_ms(600);
            struct pollfd pfdu = { .fd = (int)ufd, .events = 1 };
            poll(&pfdu, 1, 3000);
            n = read((int)ufd, buf, sizeof(buf));
            int empty_chains = 0, badu = 0;
            size_t off = 0;
            while (n > 0 && off + 8 <= (size_t)n) {
                struct rec r;
                __builtin_memcpy(&r, buf + off, 8);
                if (r.size < 8 || off + r.size > (size_t)n) {
                    badu++;
                    break;
                }
                if (r.type == PERF_RECORD_SAMPLE && r.misc == MISC_USER) {
                    if (r.size == 24) {
                        uint64_t nr;
                        __builtin_memcpy(&nr, buf + off + 16, 8);
                        if (nr == 0)
                            empty_chains++;
                        else
                            badu++;
                    } else {
                        badu++;
                    }
                }
                off += r.size;
            }
            __builtin_printf("  uchain read=%ld empty=%d bad=%d\n", n,
                             empty_chains, badu);
            check("chain-user-empty", empty_chains > 0 && badu == 0);
            close((int)ufd);
        }
    }

    // ---- 6. error matrix ----
    struct pattr b;
    int ok = 1;

    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_HARDWARE; /* unsupported class */
    b.sample_period = 1;
    ok &= pe_open(&b, 0, -1, -1, 0) == -1 && errno == 22;

    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_SOFTWARE;
    b.sample_period = 1;
    b.sample_type = PERF_SAMPLE_IP | (1ULL << 1); /* extra sample bit */
    ok &= pe_open(&b, 0, -1, -1, 0) == -1 && errno == 22;

    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_SOFTWARE;
    b.sample_period = 1000; /* freq mode reuses this field */
    b.flags1 = 1u << 10;    /* freq */
    ok &= pe_open(&b, 0, -1, -1, 0) == -1 && errno == 22;

    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_SOFTWARE;
    b.sample_period = 1;
    b.flags1 = 1u << 1; /* inherit */
    ok &= pe_open(&b, 0, -1, -1, 0) == -1 && errno == 22;

    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_SOFTWARE;
    b.sample_period = 1;
    b.read_format = 1; /* PERF_FORMAT_TOTAL_TIME_ENABLED */
    ok &= pe_open(&b, 0, -1, -1, 0) == -1 && errno == 22;

    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_SOFTWARE;
    b.sample_period = 0;
    ok &= pe_open(&b, 0, -1, -1, 0) == -1 && errno == 22;

    /* both wildcards: pid=-1 cpu=-1 */
    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_SOFTWARE;
    b.sample_period = 1;
    ok &= pe_open(&b, -1, -1, -1, 0) == -1 && errno == 22;

    /* event group requested via group_fd */
    ok &= pe_open(&a, 0, -1, 0, 0) == -1 && errno == 22;

    /* cpu out of range (guest has 2 cpus in the smoke) */
    ok &= pe_open(&a, -1, 9, -1, 0) == -1 && errno == 22;

    /* dead pid: ESRCH */
    ok &= pe_open(&a, 99991, -1, -1, 0) == -1 && errno == 3;

    /* bad attr pointer: EFAULT */
    ok &= pe_open((struct pattr *)0x10, 0, -1, -1, 0) == -1 && errno == 14;

    check("err-matrix", ok);

    // ---- 6. close + reopen (registry hygiene) ----
    __builtin_memset(&a, 0, sizeof(a));
    a.type = PERF_TYPE_SOFTWARE;
    a.size = 64;
    a.config = PERF_COUNT_SW_TASK_CLOCK;
    a.sample_period = 40000000u;
    a.sample_type = PERF_SAMPLE_IP;
    fd = pe_open(&a, 0, -1, -1, 0);
    check("close-reuse", fd >= 0);
    if (fd >= 0)
        close((int)fd);

    __builtin_printf("PERF_PROBE RESULT: %s\n", fails ? "FAIL" : "PASS");
    return fails ? 1 : 0;
}
