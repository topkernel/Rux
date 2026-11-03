/*
 * tr2-proof-init.c — dfx-trace round 2 serial proof driver (PID 1).
 *
 * Round-2 flow on the canonical tracefs mount (x86_64+KVM):
 *
 *   P0  boot state: set_event and tracing_on BEFORE any userspace
 *       write (boot A: empty/0; boot B with dfx=schedtrace: the sched
 *       family pre-armed by the switch alias + tracing_on 1).
 *   P1  layout: the round-2 files — snapshot, per_cpu list.
 *   P2  kmem family: formats (kmalloc, mm_page_alloc), set_event
 *       "kmem" group arming, the group enable reads 1.
 *   P3a kmem record: arm kmem, fork+IO workload, trace shows
 *       kmalloc/kfree/mm_page_alloc/mm_page_free lines in the
 *       reference geometry; call_site symbolized.
 *   P3b sched wake pair: sched family armed; sched_waking AND
 *       sched_wakeup lines both present (the round-2 sites), the
 *       waking count >= the wakeup count (a waking with no matching
 *       wakeup is a refused wake — the census discrimination).
 *   P3c snapshot: save (write 1) with records buffered, saved view
 *       non-empty + live trace restarted to header-only, clear
 *       (write 0) empties the saved view.
 *   P4  enabled-vs-disabled differentials, events-off baseline:
 *       fork loops with the kmem family armed (the allocator hot
 *       path), with the sched family armed, and the round-1
 *       zero-read loop with raw_syscalls armed.
 *
 * Build: gcc -static -O2 -o init run/tr2-proof-init.c
 * Image: mkfs.ext4 + debugfs write as /init
 * Boot:  -append "root=/dev/vda rw init=/init console=ttyS0 [dfx=...]"
 */

#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <time.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <errno.h>

static const char *T = "/sys/kernel/tracing";

static void say(const char *s) { write(1, s, strlen(s)); }

static void dump_file(const char *path, long max_bytes)
{
    char buf[2048];
    int fd = open(path, O_RDONLY);
    say("\n--- ");
    say(path);
    say(" ---\n");
    if (fd < 0) {
        say("(open failed)\n");
        return;
    }
    long total = 0;
    ssize_t r;
    while (total < max_bytes && (r = read(fd, buf, sizeof(buf))) > 0) {
        if (total + r > max_bytes)
            r = max_bytes - total;
        write(1, buf, r);
        total += r;
    }
    close(fd);
    if (total >= max_bytes)
        say("...\n(truncated)\n");
}

static void put(const char *path, const char *text)
{
    int fd = open(path, O_WRONLY);
    if (fd < 0) {
        say("(write open failed: ");
        say(path);
        say(")\n");
        return;
    }
    write(fd, text, strlen(text));
    close(fd);
}

static long long now_ns(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1000000000LL + t.tv_nsec;
}

/* fork/exit/wait loop: allocates (task state, kernel objects), wakes,
 * switches — exercises the kmem and sched families end to end. */
static long long bench_forks(int n)
{
    long long t0 = now_ns();
    for (int i = 0; i < n; i++) {
        pid_t p = fork();
        if (p == 0)
            _exit(0);
        if (p > 0)
            waitpid(p, NULL, 0);
    }
    return now_ns() - t0;
}

/* timed zero-read syscall loop: N reads of 8K from /dev/zero */
static long long bench_read(int reps)
{
    char buf[8192];
    long long t0 = now_ns();
    int fd = open("/dev/zero", O_RDONLY);
    long total = 0;
    for (int i = 0; i < reps; i++)
        total += read(fd, buf, sizeof(buf));
    close(fd);
    if (total < 0)
        return -1;
    return now_ns() - t0;
}

/* count lines containing `needle` in a tracefs file; optionally print
 * up to `samples` whole matching lines */
static long count_lines(const char *path, const char *needle, int samples)
{
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return -1;
    static char big[400000];
    ssize_t r = read(fd, big, sizeof(big) - 1);
    close(fd);
    if (r < 0)
        r = 0;
    big[r] = 0;
    long count = 0;
    const char *p = big;
    int shown = 0;
    while ((p = strstr(p, needle)) != NULL) {
        if (shown < samples) {
            const char *ls = p;
            while (ls > big && ls[-1] != '\n')
                ls--;
            const char *eol = strchr(p, '\n');
            if (eol) {
                write(1, "    > ", 6);
                write(1, ls, (eol - ls) + 1);
                shown++;
            }
        }
        count++;
        p++;
    }
    return count;
}

int main(void)
{
    say("===TR2-PROOF-INIT-START===\n");

    /* P0: boot state BEFORE any userspace write (the alias proof on
     * boot B: dfx=schedtrace pre-arms the sched family + tracing_on) */
    say("\n[P0] boot state:\n");
    dump_file("/sys/kernel/tracing/tracing_on", 64);
    dump_file("/sys/kernel/tracing/set_event", 2048);

    /* P1: the round-2 layout */
    say("\n[P1] round-2 files:\n");
    struct stat st;
    say(stat("/sys/kernel/tracing/snapshot", &st) == 0
            ? "OK   snapshot\n"
            : "MISS snapshot\n");
    dump_file("/sys/kernel/tracing/snapshot", 512);

    /* P2: the kmem family catalog and formats */
    dump_file("/sys/kernel/tracing/events/kmem/kmalloc/format", 2048);
    dump_file("/sys/kernel/tracing/events/kmem/mm_page_alloc/format", 2048);
    dump_file("/sys/kernel/tracing/events/kmem/mm_page_free/format", 1024);
    put("/sys/kernel/tracing/set_event", "kmem");
    say("\n[P2] after 'echo kmem > set_event':\n");
    dump_file("/sys/kernel/tracing/set_event", 512);
    dump_file("/sys/kernel/tracing/events/kmem/enable", 16);
    put("/sys/kernel/tracing/set_event", "!kmem");
    say("[P2] after disarm:\n");
    dump_file("/sys/kernel/tracing/set_event", 512);

    /* P3a: kmem record proof */
    put("/sys/kernel/tracing/set_event", "kmem");
    put("/sys/kernel/tracing/tracing_on", "1");
    put("/sys/kernel/tracing/trace_marker", "KMEM-PROOF-START");
    bench_forks(6);
    {
        int fd = open("/kmemtrigger", O_WRONLY | O_CREAT | O_TRUNC, 0644);
        char buf[4096];
        memset(buf, 'x', sizeof(buf));
        for (int i = 0; i < 8; i++)
            write(fd, buf, sizeof(buf));
        fsync(fd);
        close(fd);
    }
    put("/sys/kernel/tracing/trace_marker", "KMEM-PROOF-END");
    put("/sys/kernel/tracing/tracing_on", "0");
    say("\n[P3a] kmem line counts:\n");
    {
        const char *classes[] = { "kmalloc:", "kfree:", "mm_page_alloc:",
                                  "mm_page_free:" };
        long counts[4];
        for (unsigned c = 0; c < 4; c++) {
            counts[c] = count_lines("/sys/kernel/tracing/trace", classes[c], 2);
            char msg[96];
            int n = snprintf(msg, sizeof(msg), "%-16s %ld\n", classes[c], counts[c]);
            write(1, msg, n);
        }
        long syms = count_lines("/sys/kernel/tracing/trace", "call_site=rux::", 0);
        char msg[96];
        int n = snprintf(msg, sizeof(msg),
                         "symbolized origins: %ld (expect > 0)\n", syms);
        write(1, msg, n);
        n = snprintf(msg, sizeof(msg),
                     "KMEMDBG: alloc=%ld free=%ld page_alloc=%ld page_free=%ld origin=%ld\n",
                     counts[0], counts[1], counts[2], counts[3], syms);
        write(1, msg, n);
    }
    put("/sys/kernel/tracing/events/enable", "0");
    put("/sys/kernel/tracing/trace", "\n");

    /* P3b: the sched wake pair */
    put("/sys/kernel/tracing/set_event", "sched");
    put("/sys/kernel/tracing/tracing_on", "1");
    bench_forks(6);
    {
        char buf[8192];
        int fd = open("/dev/zero", O_RDONLY);
        for (int i = 0; i < 40; i++)
            read(fd, buf, sizeof(buf));
        close(fd);
    }
    put("/sys/kernel/tracing/tracing_on", "0");
    say("\n[P3b] wake-pair line counts:\n");
    {
        long waking = count_lines("/sys/kernel/tracing/trace", "sched_waking:", 2);
        long wakeup = count_lines("/sys/kernel/tracing/trace", "sched_wakeup:", 1);
        long wnew = count_lines("/sys/kernel/tracing/trace", "sched_wakeup_new:", 1);
        char msg[128];
        int n = snprintf(msg, sizeof(msg),
                         "WAKEDBG: waking=%ld wakeup=%ld wakeup_new=%ld"
                         " (expect all > 0, waking >= wakeup)\n",
                         waking, wakeup, wnew);
        write(1, msg, n);
    }

    /* P3c: snapshot round-trip */
    put("/sys/kernel/tracing/tracing_on", "1");
    bench_forks(4);
    put("/sys/kernel/tracing/tracing_on", "0");
    put("/sys/kernel/tracing/snapshot", "1");
    say("\n[P3c] after snapshot save:\n");
    {
        long saved = count_lines("/sys/kernel/tracing/snapshot", "sched_", 2);
        struct stat stt;
        stat("/sys/kernel/tracing/trace", &stt);
        char msg[128];
        int n = snprintf(msg, sizeof(msg),
                         "SNAPDBG: saved_sched_lines=%ld live_trace_bytes=%ld"
                         " (expect saved > 0, live == header-only)\n",
                         saved, (long)stt.st_size);
        write(1, msg, n);
        dump_file("/sys/kernel/tracing/snapshot", 600);
    }
    put("/sys/kernel/tracing/snapshot", "0");
    {
        long after = count_lines("/sys/kernel/tracing/snapshot", "sched_", 0);
        char msg[96];
        int n = snprintf(msg, sizeof(msg),
                         "SNAPDBG: after_clear=%ld (expect 0)\n", after);
        write(1, msg, n);
    }
    put("/sys/kernel/tracing/events/enable", "0");
    put("/sys/kernel/tracing/trace", "\n");

    /* P4: differentials (events-off baseline, alternating reps) */
    say("\n[P4] fork-loop differential (ns per 150 forks):\n");
    put("/sys/kernel/tracing/events/enable", "0");
    put("/sys/kernel/tracing/tracing_on", "1");
    long long kf_off[3], kf_kmem[3], kf_sched[3];
    for (int rep = 0; rep < 3; rep++) {
        put("/sys/kernel/tracing/events/enable", "0");
        kf_off[rep] = bench_forks(150);
        put("/sys/kernel/tracing/set_event", "kmem");
        kf_kmem[rep] = bench_forks(150);
        put("/sys/kernel/tracing/events/enable", "0");
        put("/sys/kernel/tracing/set_event", "sched");
        kf_sched[rep] = bench_forks(150);
    }
    put("/sys/kernel/tracing/events/enable", "0");
    {
        long long min3(long long *v)
        {
            long long m = v[0];
            for (int i = 1; i < 3; i++)
                if (v[i] < m)
                    m = v[i];
            return m;
        }
        long long off = min3(kf_off), km = min3(kf_kmem), sc = min3(kf_sched);
        char msg[320];
        int n = snprintf(msg, sizeof(msg),
                         "forks off : %lld %lld %lld (min %lld)\n"
                         "forks kmem: %lld %lld %lld (min %lld, %lld ns/fork extra)\n"
                         "forks sched: %lld %lld %lld (min %lld, %lld ns/fork extra)\n",
                         kf_off[0], kf_off[1], kf_off[2], off,
                         kf_kmem[0], kf_kmem[1], kf_kmem[2], km, (km - off) / 150,
                         kf_sched[0], kf_sched[1], kf_sched[2], sc, (sc - off) / 150);
        write(1, msg, n);
    }

    say("\n[P4] read-loop differential (ns per 4000x8K reads):\n");
    {
        long long off1, on1, off2, on2, off3, on3;
        put("/sys/kernel/tracing/events/enable", "0");
        off1 = bench_read(4000);
        put("/sys/kernel/tracing/set_event", "raw_syscalls");
        on1 = bench_read(4000);
        put("/sys/kernel/tracing/events/enable", "0");
        off2 = bench_read(4000);
        put("/sys/kernel/tracing/set_event", "raw_syscalls");
        on2 = bench_read(4000);
        put("/sys/kernel/tracing/events/enable", "0");
        off3 = bench_read(4000);
        put("/sys/kernel/tracing/set_event", "raw_syscalls");
        on3 = bench_read(4000);
        put("/sys/kernel/tracing/events/enable", "0");
        long long offmin = off1 < off2 ? (off1 < off3 ? off1 : off3)
                                       : (off2 < off3 ? off2 : off3);
        long long onmin = on1 < on2 ? (on1 < on3 ? on1 : on3)
                                    : (on2 < on3 ? on2 : on3);
        char msg[256];
        int n = snprintf(msg, sizeof(msg),
                         "off: %lld %lld %lld (min %lld)\n"
                         "on : %lld %lld %lld (min %lld)\n"
                         "delta(min) %lld ns = %lld ns/read extra (sys_enter+sys_exit)\n",
                         off1, off2, off3, offmin, on1, on2, on3, onmin,
                         onmin - offmin, (onmin - offmin) / 4000);
        write(1, msg, n);
    }
    put("/sys/kernel/tracing/tracing_on", "0");
    put("/sys/kernel/tracing/events/enable", "0");

    say("\n===TR2-PROOF-INIT-DONE===\n");

    while (1) {
        struct timespec ts = { 60, 0 };
        nanosleep(&ts, NULL);
    }
    return 0;
}
