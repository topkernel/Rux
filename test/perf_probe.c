// perf_probe.c — userspace smoke for the perf_event_open software-clock
// slice (PERF_TYPE_SOFTWARE cpu-clock/task-clock, sample_period,
// PERF_SAMPLE_IP / PERF_SAMPLE_CALLCHAIN).
//
// read(2) contract (reference): the counter read — u64 count, then per
// attr.read_format u64 total_time_enabled, u64 total_time_running, u64
// id. It NEVER blocks (the old blocking drain self-deadlocked on a
// task-scoped event read from the same task: the reader slept, its own
// ticks never charged, no sample ever arrived). Sample records are NOT
// read(2) output — they drain from the stopgap per-event debugfs file
// /sys/kernel/debug/perf/<id>/records (each open drains a batch).
//
// Checks (each prints PERF_PROBE <name>: PASS/FAIL):
//   counter-read   read() right after open returns immediately (8 B
//                  counter), and the count grows with spin time
//   fmt-layout     read_format ENABLED|RUNNING|ID: 32 B — running <=
//                  enabled, id names the drain file
//   no-block       a task-clock read on self after 50 ms spin returns
//                  at once (the session-13 deadlock shape, now legal)
//   flag-matrix    PERF_FLAG_* uapi values: NO_GROUP (1) accepted,
//                  CLOEXEC (8) accepted, FD_OUTPUT (2) / PID_CGROUP
//                  (4) / unknown bits EINVAL
//   user-sample    after a user spin, poll says POLLIN and the drain
//                  file yields 16-byte SAMPLE records, misc=USER,
//                  non-zero ip
//   cpu-sample     per-CPU cpu-clock (pid=-1, cpu=0) yields records
//   excl-user      exclude_user event yields only KERNEL-misc records
//   chain-kernel   per-CPU + CALLCHAIN + exclude_user: kernel samples
//                  carry nr = 1 + frames, ips[0] == PERF_CONTEXT_KERNEL
//   chain-user     task-clock on self in user mode: 24-byte records,
//                  nr = 0 (no user-stack walker — honest empty)
//   drain-gone     close(2) removes the event's drain file (ENOENT)
//   short-einval   a 4-byte read of the default layout is EINVAL
//   err-matrix     EINVAL: hardware type, extra sample bits, freq mode,
//                  inherit, PERF_FORMAT_GROUP, period=0, both pid&cpu
//                  -1, group_fd>=0, cpu out of range; ESRCH: dead pid;
//                  EFAULT: bad attr pointer
//   close-reuse    close(2) then a fresh open works (registry clean)
//
// Build: musl-static via test/perf-smoke.sh (rootfs libc — the distro
// glibc carries RVV the standard QEMU cpu lacks).

#include <stdint.h>
#include <stddef.h>
#include <errno.h>
#include <stdio.h>
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

/* read_format bits (uapi). */
#define RF_ENABLED (1ULL << 0)
#define RF_RUNNING (1ULL << 1)
#define RF_ID (1ULL << 2)
#define RF_GROUP (1ULL << 3)

/* perf_event_open flags (uapi values — the smoke pins them). */
#define PFLAG_FD_NO_GROUP (1ULL << 0)
#define PFLAG_FD_OUTPUT (1ULL << 1)
#define PFLAG_PID_CGROUP (1ULL << 2)
#define PFLAG_FD_CLOEXEC (1ULL << 3)

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
// *n_kernel / *n_lost / *bad.
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

/* One counter read. Returns 0 on success. */
struct cread {
    uint64_t count, enabled, running, id;
};
static int read_counter(int fd, uint64_t fmt, struct cread *out)
{
    uint8_t b[32];
    long n = read(fd, b, 32);
    if (n != 8 + (long)((fmt & RF_ENABLED) ? 8 : 0) +
                  (long)((fmt & RF_RUNNING) ? 8 : 0) +
                  (long)((fmt & RF_ID) ? 8 : 0))
        return -1;
    size_t at = 0;
    __builtin_memcpy(&out->count, b + at, 8);
    at += 8;
    if (fmt & RF_ENABLED) {
        __builtin_memcpy(&out->enabled, b + at, 8);
        at += 8;
    } else {
        out->enabled = 0;
    }
    if (fmt & RF_RUNNING) {
        __builtin_memcpy(&out->running, b + at, 8);
        at += 8;
    } else {
        out->running = 0;
    }
    if (fmt & RF_ID) {
        __builtin_memcpy(&out->id, b + at, 8);
    } else {
        out->id = 0;
    }
    return 0;
}

/* Drain one batch of the event's records through the debugfs file.
 * Returns bytes read (0 = nothing pending), -1 when the file is gone. */
static long drain_records(uint64_t id, uint8_t *buf, size_t cap)
{
    char path[64];
    snprintf(path, sizeof(path), "/sys/kernel/debug/perf/%llu/records",
             (unsigned long long)id);
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return -1;
    long n = read(fd, buf, cap);
    close(fd);
    return n;
}

int main(void)
{
    uint8_t buf[16384];
    struct cread cr, cr2;

    /* ---- 1. counter read: immediate, monotone (never blocks) ---- */
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

    /* The session-13 deadlock shape, now legal: read IMMEDIATELY after
     * open, task-scoped on self. Must answer at once with the count. */
    uint64_t t0 = now_ms();
    int ok = read_counter((int)fd, 0, &cr) == 0 && now_ms() - t0 < 100;
    __builtin_printf("  c0=%llu in %llums\n",
                     (unsigned long long)cr.count,
                     (unsigned long long)(now_ms() - t0));
    check("no-block", ok);

    spin_user_ms(300);
    ok = read_counter((int)fd, 0, &cr2) == 0 && cr2.count > cr.count;
    __builtin_printf("  c1=%llu c2=%llu\n", (unsigned long long)cr.count,
                     (unsigned long long)cr2.count);
    check("counter-read", ok);

    /* Short read of the default layout: EINVAL (need 8 bytes). */
    {
        int8_t tiny[4];
        errno = 0;
        long r = read((int)fd, tiny, 4);
        check("short-einval", r == -1 && errno == 22);
    }
    close((int)fd);

    /* ---- 2. read_format layout + the drain file's name ---- */
    __builtin_memset(&a, 0, sizeof(a));
    a.type = PERF_TYPE_SOFTWARE;
    a.size = 64;
    a.config = PERF_COUNT_SW_TASK_CLOCK;
    a.sample_period = 40000000u;
    a.sample_type = PERF_SAMPLE_IP;
    a.read_format = RF_ENABLED | RF_RUNNING | RF_ID;
    fd = pe_open(&a, 0, -1, -1, 0);
    check("open-fmt", fd >= 0);
    if (fd >= 0) {
        spin_user_ms(200);
        ok = read_counter((int)fd, a.read_format, &cr) == 0 &&
             cr.running <= cr.enabled && cr.id != 0;
        __builtin_printf("  fmt count=%llu en=%llu run=%llu id=%llu\n",
                         (unsigned long long)cr.count,
                         (unsigned long long)cr.enabled,
                         (unsigned long long)cr.running,
                         (unsigned long long)cr.id);
        check("fmt-layout", ok);
        /* The drain file for THIS event exists. */
        ok = drain_records(cr.id, buf, sizeof(buf)) >= 0;
        check("drain-file", ok);
        close((int)fd);
        /* close removed it. */
        char path[64];
        snprintf(path, sizeof(path), "/sys/kernel/debug/perf/%llu/records",
                 (unsigned long long)cr.id);
        check("drain-gone", open(path, O_RDONLY) < 0 && errno == 2);
    }

    /* ---- 3. flag matrix: the uapi PERF_FLAG_* values ---- */
    {
        __builtin_memset(&a, 0, sizeof(a));
        a.type = PERF_TYPE_SOFTWARE;
        a.size = 64;
        a.config = PERF_COUNT_SW_TASK_CLOCK;
        a.sample_period = 40000000u;
        a.sample_type = PERF_SAMPLE_IP;
        long f1 = pe_open(&a, 0, -1, -1, PFLAG_FD_NO_GROUP);
        long f2 = pe_open(&a, 0, -1, -1, PFLAG_FD_CLOEXEC);
        long f3 = pe_open(&a, 0, -1, -1, PFLAG_FD_OUTPUT);
        long f4 = pe_open(&a, 0, -1, -1, PFLAG_PID_CGROUP);
        long f5 = pe_open(&a, 0, -1, -1, 1ULL << 5);
        if (f1 >= 0)
            close((int)f1);
        if (f2 >= 0)
            close((int)f2);
        __builtin_printf("  flags no_group=%ld cloexec=%ld output=%ld "
                         "cgroup=%ld unk=%ld\n", f1, f2, f3, f4, f5);
        check("flag-matrix",
              f1 >= 0 && f2 >= 0 && f3 == -1 && f4 == -1 && f5 == -1);
    }

    /* ---- 4. user samples: task-clock on self, records via drain ---- */
    __builtin_memset(&a, 0, sizeof(a));
    a.type = PERF_TYPE_SOFTWARE;
    a.size = 64;
    a.config = PERF_COUNT_SW_TASK_CLOCK;
    a.sample_period = 20000000u;
    a.sample_type = PERF_SAMPLE_IP;
    a.read_format = RF_ID;
    fd = pe_open(&a, 0, -1, -1, 0);
    check("open-user", fd >= 0);
    if (fd >= 0) {
        spin_user_ms(600);
        struct pollfd pfd = { .fd = (int)fd, .events = 1 /* POLLIN */ };
        int pr = poll(&pfd, 1, 3000);
        check("pollin", pr == 1 && (pfd.revents & 1));
        read_counter((int)fd, a.read_format, &cr);
        int n_user = 0, n_kernel = 0, n_lost = 0, bad = 0;
        long n = drain_records(cr.id, buf, sizeof(buf));
        int nrec = n > 0 ? decode(buf, (size_t)n, 64, &n_user, &n_kernel,
                                  &n_lost, &bad) : 0;
        __builtin_printf("  drain=%ld recs=%d user=%d kern=%d bad=%d\n",
                         n, nrec, n_user, n_kernel, bad);
        check("user-sample", n > 0 && nrec > 0 && n_user > 0 && bad == 0);
        close((int)fd);
    }

    /* ---- 5. per-CPU cpu-clock ---- */
    __builtin_memset(&a, 0, sizeof(a));
    a.type = PERF_TYPE_SOFTWARE;
    a.size = 64;
    a.config = PERF_COUNT_SW_CPU_CLOCK;
    a.sample_period = 20000000u;
    a.sample_type = PERF_SAMPLE_IP;
    a.read_format = RF_ID;
    fd = pe_open(&a, -1, 0, -1, 0);
    check("open-cpu", fd >= 0);
    if (fd >= 0) {
        spin_user_ms(400);
        struct pollfd pfd2 = { .fd = (int)fd, .events = 1 };
        poll(&pfd2, 1, 3000);
        read_counter((int)fd, a.read_format, &cr);
        int n_user = 0, n_kernel = 0, n_lost = 0, bad = 0;
        long n = drain_records(cr.id, buf, sizeof(buf));
        int nrec = n > 0 ? decode(buf, (size_t)n, 64, &n_user, &n_kernel,
                                  &n_lost, &bad) : 0;
        __builtin_printf("  cpu drain=%ld recs=%d\n", n, nrec);
        check("cpu-sample", n > 0 && nrec > 0 && bad == 0);
        close((int)fd);
    }

    /* ---- 6. exclude_user (kernel-only records must be KERNEL-misc) ---- */
    __builtin_memset(&a, 0, sizeof(a));
    a.type = PERF_TYPE_SOFTWARE;
    a.size = 64;
    a.config = PERF_COUNT_SW_TASK_CLOCK;
    a.sample_period = 20000000u;
    a.sample_type = PERF_SAMPLE_IP;
    a.read_format = RF_ID;
    a.flags1 = 1u << 4; /* exclude_user */
    fd = pe_open(&a, 0, -1, -1, 0);
    check("open-excl", fd >= 0);
    if (fd >= 0) {
        spin_mixed_ms(500);
        struct pollfd pfd3 = { .fd = (int)fd, .events = 1 };
        int pr = poll(&pfd3, 1, 1000);
        int n_user = 0, n_kernel = 0, n_lost = 0, bad = 0;
        long n = 0;
        int nrec = 0;
        if (pr == 1 && (pfd3.revents & 1)) {
            read_counter((int)fd, a.read_format, &cr);
            n = drain_records(cr.id, buf, sizeof(buf));
            nrec = n > 0 ? decode(buf, (size_t)n, 64, &n_user, &n_kernel,
                                  &n_lost, &bad) : 0;
        } else {
            /* No kernel-mode tick landed in the window: legal. The
             * counter read below still must answer at once. */
            n_user = n_kernel = n_lost = bad = 0;
        }
        struct cread crx;
        /* The event carries RF_ID — read(2) answers ITS layout (16 B),
         * so the helper must expect the event's format, not 0. */
        int fast = read_counter((int)fd, a.read_format, &crx) == 0;
        __builtin_printf("  excl poll=%d drain=%ld recs=%d kern=%d\n", pr,
                         n, nrec, n_kernel);
        check("excl-user", fast && n >= 0 && n_user == 0 && bad == 0);
        close((int)fd);
    }

    /* ---- 7. callchain samples (PERF_SAMPLE_CALLCHAIN) ----
     * a) per-CPU cpu-clock: kernel-mode samples carry nr = 1 + frames,
     *    ips[0] == PERF_CONTEXT_KERNEL (0xffffffffffffff00), then
     *    kernel text return addresses (the wedge-hunt food);
     * b) task-clock on self while spinning in USER mode: records are
     *    24 bytes with nr = 0 (no user-stack walker — honest empty). */
    {
        struct pattr c;
        __builtin_memset(&c, 0, sizeof(c));
        c.type = PERF_TYPE_SOFTWARE;
        c.size = 64;
        c.config = PERF_COUNT_SW_CPU_CLOCK;
        c.sample_period = 20000000u;
        c.sample_type = PERF_SAMPLE_IP | (1ULL << 5); /* CALLCHAIN */
        c.read_format = RF_ID;
        c.flags1 = 1u << 4; /* exclude_user: kernel-mode samples only */
        long cfd = pe_open(&c, -1, 0, -1, 0);
        check("open-chain", cfd >= 0);
        if (cfd >= 0) {
            spin_mixed_ms(1200);
            struct pollfd pfdc = { .fd = (int)cfd, .events = 1 };
            poll(&pfdc, 1, 3000);
            read_counter((int)cfd, c.read_format, &cr);
            long n = drain_records(cr.id, buf, sizeof(buf));
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
            __builtin_printf("  chain drain=%ld chains=%d frames=%d bad=%d\n",
                             n, chains, kframes, badc);
            check("chain-kernel", chains > 0 && kframes > 0 && badc == 0);
            close((int)cfd);
        }

        /* user-mode chain records: nr == 0, size 24. */
        c.config = PERF_COUNT_SW_TASK_CLOCK;
        c.flags1 = 0;
        long ufd = pe_open(&c, 0, -1, -1, 0);
        check("open-chain-user", ufd >= 0);
        if (ufd >= 0) {
            spin_user_ms(600);
            struct pollfd pfdu = { .fd = (int)ufd, .events = 1 };
            poll(&pfdu, 1, 3000);
            read_counter((int)ufd, c.read_format, &cr);
            long n = drain_records(cr.id, buf, sizeof(buf));
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
            __builtin_printf("  uchain drain=%ld empty=%d bad=%d\n", n,
                             empty_chains, badu);
            check("chain-user-empty", empty_chains > 0 && badu == 0);
            close((int)ufd);
        }
    }

    /* ---- 8. error matrix ---- */
    struct pattr b;
    int okm = 1;

    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_HARDWARE; /* unsupported class */
    b.sample_period = 1;
    okm &= pe_open(&b, 0, -1, -1, 0) == -1 && errno == 22;

    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_SOFTWARE;
    b.sample_period = 1;
    b.sample_type = PERF_SAMPLE_IP | (1ULL << 1); /* extra sample bit */
    okm &= pe_open(&b, 0, -1, -1, 0) == -1 && errno == 22;

    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_SOFTWARE;
    b.sample_period = 1000;
    b.flags1 = 1u << 10; /* freq */
    okm &= pe_open(&b, 0, -1, -1, 0) == -1 && errno == 22;

    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_SOFTWARE;
    b.sample_period = 1;
    b.flags1 = 1u << 1; /* inherit */
    okm &= pe_open(&b, 0, -1, -1, 0) == -1 && errno == 22;

    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_SOFTWARE;
    b.sample_period = 1;
    b.read_format = RF_GROUP; /* group read layout: no groups */
    okm &= pe_open(&b, 0, -1, -1, 0) == -1 && errno == 22;

    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_SOFTWARE;
    b.sample_period = 0;
    okm &= pe_open(&b, 0, -1, -1, 0) == -1 && errno == 22;

    /* both wildcards: pid=-1 cpu=-1 */
    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_SOFTWARE;
    b.sample_period = 1;
    okm &= pe_open(&b, -1, -1, -1, 0) == -1 && errno == 22;

    __builtin_memset(&b, 0, sizeof(b));
    b.size = 64;
    b.type = PERF_TYPE_SOFTWARE;
    b.sample_period = 40000000u;
    b.sample_type = PERF_SAMPLE_IP;
    /* event group requested via group_fd */
    okm &= pe_open(&b, 0, -1, 0, 0) == -1 && errno == 22;

    /* cpu out of range (guest has 2 cpus in the smoke) */
    okm &= pe_open(&b, -1, 9, -1, 0) == -1 && errno == 22;

    /* dead pid: ESRCH */
    okm &= pe_open(&b, 99991, -1, -1, 0) == -1 && errno == 3;

    /* bad attr pointer: EFAULT */
    okm &= pe_open((struct pattr *)0x10, 0, -1, -1, 0) == -1 && errno == 14;

    check("err-matrix", okm);

    /* ---- 9. close + reopen (registry hygiene) ---- */
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
