/*
 * tr1-proof-init.c — dfx-trace round 1 serial proof driver (PID 1).
 *
 * A tiny static x86_64 init that walks the reference-style tracefs flow
 * against the CANONICAL mount and prints everything to /dev/console:
 *
 *   P1  layout: the /sys/kernel/tracing file set; the debugfs alias.
 *   P2  event catalog; the new events' ids; a format file.
 *   P3  record: enable sched+irq+printk+raw_syscalls (set_event one
 *       token per write, the reference grammar), tracing_on, marker
 *       brackets, a fork/read workload, trace + trace_pipe + stats.
 *   P4  options: defaults; context-info off drops the header.
 *   P5  enabled-vs-disabled differential: timed zero-read syscall
 *       loops (clock_gettime, ns) with all events off vs raw_syscalls
 *       armed, alternating reps.
 *
 * Build: gcc -static -O2 -o init run/tr1-proof-init.c
 * Image: mkfs.ext4 + debugfs write as /init
 * Boot:  -append "root=/dev/vda rw init=/init console=ttyS0"
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
        say("(open failed: ");
        char e[32];
        int n = snprintf(e, sizeof(e), "%d)", errno);
        write(1, e, n);
        say("\n");
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
        say("(write open failed: "); say(path); say(")\n");
        return;
    }
    write(fd, text, strlen(text));
    close(fd);
}

/* timed zero-read syscall loop: N reads of 8K from /dev/zero */
static long long bench_read(int reps)
{
    char buf[8192];
    struct timespec t0, t1;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    int fd = open("/dev/zero", O_RDONLY);
    long total = 0;
    for (int i = 0; i < reps; i++)
        total += read(fd, buf, sizeof(buf));
    close(fd);
    clock_gettime(CLOCK_MONOTONIC, &t1);
    if (total < 0)
        return -1;
    return (t1.tv_sec - t0.tv_sec) * 1000000000LL +
           (t1.tv_nsec - t0.tv_nsec);
}

/* fork/exit workload: exercises sched_switch + sched_process_fork/exit */
static void workload_forks(int n)
{
    for (int i = 0; i < n; i++) {
        pid_t p = fork();
        if (p == 0)
            _exit(0);
        if (p > 0)
            waitpid(p, NULL, 0);
    }
}

int main(void)
{
    say("===TR1-PROOF-INIT-START===\n");

    /* P1: the canonical layout + the debugfs alias (stat markers) */
    say("\n[P1] /sys/kernel/tracing layout:\n");
    const char *p1_files[] = {
        "/sys/kernel/tracing/tracing_on", "/sys/kernel/tracing/trace",
        "/sys/kernel/tracing/trace_pipe", "/sys/kernel/tracing/trace_marker",
        "/sys/kernel/tracing/available_events", "/sys/kernel/tracing/set_event",
        "/sys/kernel/tracing/buffer_size_kb", "/sys/kernel/tracing/options",
        "/sys/kernel/tracing/events", "/sys/kernel/tracing/per_cpu",
        "/sys/kernel/debug/tracing/tracing_on",
        "/sys/kernel/debug/tracing/available_events",
    };
    for (unsigned i = 0; i < sizeof(p1_files) / sizeof(p1_files[0]); i++) {
        struct stat st;
        int ok = stat(p1_files[i], &st) == 0;
        say(ok ? "OK   " : "MISS ");
        say(p1_files[i]);
        say(ok ? (S_ISDIR(st.st_mode) ? " (dir)\n" : "\n") : "\n");
    }
    dump_file("/sys/kernel/tracing/tracing_on", 64);
    dump_file("/sys/kernel/tracing/available_tracers", 64);
    dump_file("/sys/kernel/tracing/current_tracer", 64);
    dump_file("/sys/kernel/tracing/buffer_size_kb", 64);

    /* P2: catalog + the new events */
    dump_file("/sys/kernel/tracing/available_events", 4096);
    dump_file("/sys/kernel/tracing/events/irq/irq_handler_entry/id", 32);
    dump_file("/sys/kernel/tracing/events/irq/irq_handler_exit/id", 32);
    dump_file("/sys/kernel/tracing/events/printk/console/id", 32);
    dump_file("/sys/kernel/tracing/events/sched/sched_migrate_task/id", 32);
    dump_file("/sys/kernel/tracing/events/irq/irq_handler_entry/format", 2048);
    dump_file("/sys/kernel/tracing/events/printk/console/format", 1024);
    dump_file("/sys/kernel/tracing/events/sched/sched_migrate_task/format", 2048);

    /* the reference grammar: one token per write; bare name = group */
    put("/sys/kernel/tracing/set_event", "raw_syscalls");
    say("\n[P2] after 'echo raw_syscalls > set_event':\n");
    dump_file("/sys/kernel/tracing/set_event", 256);
    dump_file("/sys/kernel/tracing/events/raw_syscalls/enable", 16);
    put("/sys/kernel/tracing/set_event", "!raw_syscalls");
    say("[P2] after disarm:\n");
    dump_file("/sys/kernel/tracing/set_event", 256);
    /* EINVAL probe: a token matching nothing */
    {
        int fd = open("/sys/kernel/tracing/set_event", O_WRONLY);
        ssize_t r = write(fd, "nosuch:event", 12);
        close(fd);
        char msg[96];
        int n = snprintf(msg, sizeof(msg),
                         "[P2] nosuch:event write returns %zd (errno %d; expect -1 EINVAL)\n",
                         r, r < 0 ? errno : 0);
        write(1, msg, n);
    }

    /* P3: record proof */
    put("/sys/kernel/tracing/set_event", "sched");
    put("/sys/kernel/tracing/set_event", "irq");
    put("/sys/kernel/tracing/set_event", "printk:console");
    put("/sys/kernel/tracing/set_event", "raw_syscalls");
    say("\n[P3] armed events:\n");
    dump_file("/sys/kernel/tracing/set_event", 512);
    put("/sys/kernel/tracing/tracing_on", "1");
    put("/sys/kernel/tracing/trace_marker", "TRACE-PROOF-START");
    say("(workload: 12 forks + a read loop + disk write/read + kmsg)\n");
    workload_forks(12);
    {
        char buf[8192];
        int fd = open("/dev/zero", O_RDONLY);
        for (int i = 0; i < 60; i++)
            read(fd, buf, sizeof(buf));
        close(fd);
    }
    /* Disk traffic: create + write + fsync + re-read a file — the
     * virtio-blk completions arrive as IRQs (irq:irq_handler_entry/exit
     * with name=virtio-pci on the x86 line path). */
    {
        int fd = open("/irqtrigger", O_WRONLY | O_CREAT | O_TRUNC, 0644);
        char buf[4096];
        memset(buf, 'x', sizeof(buf));
        for (int i = 0; i < 16; i++)
            write(fd, buf, sizeof(buf));
        fsync(fd);
        close(fd);
        fd = open("/irqtrigger", O_RDONLY);
        for (int i = 0; i < 16; i++)
            read(fd, buf, sizeof(buf));
        close(fd);
    }
    /* A printk store through /dev/kmsg (printk:console fires there —
     * devkmsg writes use the same store path). */
    {
        int fd = open("/dev/kmsg", O_WRONLY);
        if (fd >= 0) {
            write(fd, "trace-proof console trigger", 28);
            close(fd);
        } else {
            say("(/dev/kmsg not available)\n");
        }
    }
    put("/sys/kernel/tracing/trace_marker", "TRACE-PROOF-END");
    put("/sys/kernel/tracing/tracing_on", "0");
    say("\n[P3] trace (first 6 KiB):\n");
    dump_file("/sys/kernel/tracing/trace", 6144);
    say("\n[P3] line-class counts:\n");
    {
        const char *classes[] = {
            "sched_switch:", "sched_process_fork:", "sched_process_exit:",
            "sched_wakeup", "irq_handler_entry:", "irq_handler_exit:",
            "console:", "sys_enter", "sys_exit", "TRACE-PROOF",
        };
        int fd = open("/sys/kernel/tracing/trace", O_RDONLY);
        char big[200000];
        ssize_t r = read(fd, big, sizeof(big) - 1);
        close(fd);
        if (r < 0)
            r = 0;
        big[r] = 0;
        for (unsigned c = 0; c < sizeof(classes) / sizeof(classes[0]); c++) {
            long count = 0;
            const char *p = big;
            const char *first = NULL, *second = NULL;
            while ((p = strstr(p, classes[c])) != NULL) {
                if (count == 0)
                    first = p;
                else if (count == 1)
                    second = p;
                count++;
                p++;
            }
            char msg[96];
            int n = snprintf(msg, sizeof(msg), "%-24s %ld\n",
                             classes[c], count);
            write(1, msg, n);
            /* print the first two matching LINES (raw transcript proof) */
            const char *samples[2] = { first, second };
            for (int s = 0; s < 2; s++) {
                if (!samples[s])
                    continue;
                const char *eol = strchr(samples[s], '\n');
                if (!eol)
                    continue;
                const char *ls = samples[s];
                while (ls > big && ls[-1] != '\n')
                    ls--;
                write(1, "    > ", 6);
                write(1, ls, (eol - ls) + 1);
            }
        }
    }
    say("\n[P3] trace_pipe consumes:\n");
    dump_file("/sys/kernel/tracing/trace_pipe", 512);
    {
        struct stat st;
        stat("/sys/kernel/tracing/trace", &st);
        char msg[96];
        int n = snprintf(msg, sizeof(msg),
                         "(trace size after pipe read: %ld bytes)\n",
                         (long)st.st_size);
        write(1, msg, n);
    }
    dump_file("/sys/kernel/tracing/per_cpu/cpu0/stats", 512);

    /* P4: options */
    say("\n[P4] option defaults:\n");
    dump_file("/sys/kernel/tracing/options/context-info", 8);
    dump_file("/sys/kernel/tracing/options/overwrite", 8);
    dump_file("/sys/kernel/tracing/options/sym-addr", 8);
    dump_file("/sys/kernel/tracing/options/sym-offset", 8);
    put("/sys/kernel/tracing/options/context-info", "0");
    say("[P4] trace head with context-info off (no header):\n");
    dump_file("/sys/kernel/tracing/trace", 200);
    put("/sys/kernel/tracing/options/context-info", "1");

    /* P5: enabled-vs-disabled differential */
    say("\n[P5] differential (ns per 4000x8K zero-reads):\n");
    put("/sys/kernel/tracing/events/enable", "0");
    put("/sys/kernel/tracing/tracing_on", "1");
    long long off1 = bench_read(4000);
    put("/sys/kernel/tracing/set_event", "raw_syscalls");
    long long on1 = bench_read(4000);
    put("/sys/kernel/tracing/events/enable", "0");
    long long off2 = bench_read(4000);
    put("/sys/kernel/tracing/set_event", "raw_syscalls");
    long long on2 = bench_read(4000);
    put("/sys/kernel/tracing/events/enable", "0");
    long long off3 = bench_read(4000);
    put("/sys/kernel/tracing/set_event", "raw_syscalls");
    long long on3 = bench_read(4000);
    put("/sys/kernel/tracing/tracing_on", "0");
    put("/sys/kernel/tracing/events/enable", "0");
    {
        char msg[256];
        long long offmin = off1 < off2 ? (off1 < off3 ? off1 : off3) : (off2 < off3 ? off2 : off3);
        long long onmin = on1 < on2 ? (on1 < on3 ? on1 : on3) : (on2 < on3 ? on2 : on3);
        int n = snprintf(msg, sizeof(msg),
                         "off: %lld %lld %lld ns (min %lld)\n"
                         "on : %lld %lld %lld ns (min %lld)\n"
                         "delta(min) %lld ns over 4000 reads = %lld ns/read extra with sys_enter+sys_exit armed\n",
                         off1, off2, off3, offmin, on1, on2, on3, onmin,
                         onmin - offmin, (onmin - offmin) / 4000);
        write(1, msg, n);
    }
    dump_file("/sys/kernel/tracing/per_cpu/cpu0/stats", 512);

    say("\n===TR1-PROOF-INIT-DONE===\n");

    /* PID 1 stays alive so the box does not panic before the transcript
     * is flushed; the host driver's timeout ends the VM. */
    while (1) {
        struct timespec ts = { 60, 0 };
        nanosleep(&ts, NULL);
    }
    return 0;
}
