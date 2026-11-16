/* dfx_mig_probe.c — x86_64 verification probe for the dfx-migrate
 * round (runs as init on a fresh minimal ext4; host gcc, static).
 *
 * Exercises the migrated families' reachable faces and dumps the
 * tracefs surface, with ===MARK / ===TRACE markers for the host-side
 * assertions:
 *   1. three fork+exit children  -> EXITNOTIFY hunt lines (schedtrace
 *      gate) + trace_schedtrace rows in the ring
 *   2. kill(child, SIGKILL)      -> KILLTRACE hunt line (sigtrace gate)
 *   3. raise(SIGUSR1) + handler  -> signal delivery (aarch64-only frame
 *      prints; on x86 the generate path is covered by 2)
 *   4. available_events / set_event / trace dumps through tracefs
 */
#include <unistd.h>
#include <fcntl.h>
#include <signal.h>
#include <string.h>
#include <sys/wait.h>
#include <stdio.h>

static void emit(const char *s) { write(1, s, strlen(s)); }

static void dump_file(const char *path)
{
    char buf[4096];
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        emit("===TRACE: open-fail ");
        emit(path);
        emit("\n");
        return;
    }
    ssize_t n;
    while ((n = read(fd, buf, sizeof(buf))) > 0)
        write(1, buf, n);
    close(fd);
}

static void usr1_handler(int sig) { (void)sig; }

int main(void)
{
    emit("===MARK: probe-start\n");

    /* trace_marker sanity: proves tracing_on + the read path */
    {
        int fd = open("/sys/kernel/debug/tracing/trace_marker", O_WRONLY);
        if (fd >= 0) {
            ssize_t w = write(fd, "MIGMARKER-alpha\n", 16);
            (void)w;
            close(fd);
            emit("===MARK: marker-written\n");
        } else {
            emit("===MARK: marker-open-fail\n");
        }
    }

    /* 1. fork+exit children -> EXITNOTIFY rows */
    for (int i = 0; i < 3; i++) {
        pid_t p = fork();
        if (p == 0)
            _exit(0);
        int st;
        waitpid(p, &st, 0);
    }
    emit("===MARK: children-reaped\n");

    /* 2. SIGKILL to a child -> KILLTRACE (sigtrace gate) */
    {
        pid_t p = fork();
        if (p == 0) {
            pause(); /* sit until the kill lands */
            _exit(0);
        }
        kill(p, 9);
        int st;
        waitpid(p, &st, 0);
    }
    emit("===MARK: kill-delivered\n");

    /* 3. catchable signal delivery */
    signal(SIGUSR1, usr1_handler);
    raise(SIGUSR1);
    emit("===MARK: usr1-delivered\n");

    /* 4. the tracefs surface */
    // Round 5: let the armed heartbeat families fire (WW 1 Hz, SC-HB
    // 10 s, ledger series 30 s) before dumping the ring, so their trace
    // rows exist.
    sleep(35);
    emit("===TRACE: available-rux\n");
    {
        int fd = open("/sys/kernel/debug/tracing/available_events", O_RDONLY);
        char buf[8192];
        ssize_t n = fd >= 0 ? read(fd, buf, sizeof(buf) - 1) : -1;
        if (fd >= 0)
            close(fd);
        if (n > 0) {
            buf[n] = 0;
            /* print only the rux: lines */
            char *line = buf;
            while (line && *line) {
                char *nl = strchr(line, '\n');
                if (nl)
                    *nl = 0;
                if (strstr(line, "rux:"))
                    printf("%s\n", line);
                line = nl ? nl + 1 : 0;
            }
        } else {
            emit("===TRACE: available_events-unreadable\n");
        }
    }
    emit("===TRACE: set-event\n");
    dump_file("/sys/kernel/debug/tracing/set_event");
    emit("===TRACE: tracing-on\n");
    dump_file("/sys/kernel/debug/tracing/tracing_on");
    emit("===TRACE: trace-rux\n");
    {
        int fd = open("/sys/kernel/debug/tracing/trace", O_RDONLY);
        char buf[12288];
        ssize_t total = 0;
        ssize_t n;
        int nrux = 0;
        while (fd >= 0 && (n = read(fd, buf, sizeof(buf))) > 0) {
            buf[n < (ssize_t)sizeof(buf) ? n : (ssize_t)sizeof(buf) - 1] = 0;
            char *line = buf;
            /* line-by-line: count + keep the tail sample of rux: rows */
            while (line && *line) {
                char *nl = strchr(line, '\n');
                if (strstr(line, "schedtrace:") || strstr(line, "fuxstuck:") || strstr(line, "txnhang:") || strstr(line, "mmforensics:") || strstr(line, "sysring:") || strstr(line, "ohhunt:") || strstr(line, "sigtrace:") || strstr(line, "futex_miss:")) {
                    nrux++;
                    if (nrux <= 6)
                        printf("RUXROW %s\n", line);
                }
                if (!nl)
                    break;
                *nl = '\n';
                line = nl + 1;
            }
            total += n;
        }
        if (fd >= 0)
            close(fd);
        printf("===TRACE: rux-record-count %d (trace bytes %zd)\n", nrux, total);
        /* raw tail: dump the whole trace text (bounded) for the log */
        int fd2 = open("/sys/kernel/debug/tracing/trace", O_RDONLY);
        if (fd2 >= 0) {
            char b2[16384];
            ssize_t got = read(fd2, b2, sizeof(b2) - 1);
            close(fd2);
            if (got > 0) {
                b2[got] = 0;
                emit("===TRACE: rawdump-start\n");
                write(1, b2, got);
                emit("\n===TRACE: rawdump-end\n");
            }
        }
    }
    emit("===MARK: probe-end\n");
    return 0;
}
