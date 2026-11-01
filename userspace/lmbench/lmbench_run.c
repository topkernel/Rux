/* lmbench_run.c — single-case lmbench runner for the Rux rig (PID 1).
 *
 * The case comes from the kernel cmdline ("lmbench=<bench> [args]"):
 * the cmdline parser does not preserve quotes, so the init= argument
 * cannot carry it. One case per boot keeps the serial output clean
 * (concurrent kernel prints interleave with a long sweep) and
 * isolates a hanging benchmark into its own timeout.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <signal.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <sys/reboot.h>

/* Benchmarks needing a scratch file (lat_pagefault / lat_mmap /
 * bw_file_rd / bw_mmap_rd) all use this one: 16 MiB is comfortably
 * above lat_pagefault's 1 MiB minimum and above the 1m read sizes. */
#define SCRATCH "/tmp/lmbench.file"
#define SCRATCH_SZ (16UL * 1024 * 1024)

/* Client/server benchmarks: the runner starts the "<bench> -s" server
 * locally before exec'ing the client against 127.0.0.1 (the upstream
 * harness starts these servers remotely; there is no rsh path here, so
 * the runner owns the server lifetime). */
static int is_net_bench(const char *b)
{
    return !strcmp(b, "lat_tcp") || !strcmp(b, "lat_udp") ||
           !strcmp(b, "lat_connect") || !strcmp(b, "bw_tcp") ||
           !strcmp(b, "bw_udp");
}

static void make_scratch(void)
{
    int fd = open(SCRATCH, O_CREAT | O_TRUNC | O_WRONLY, 0644);
    if (fd < 0)
        return;
    /* Materialize REAL blocks, not a sparse file: ftruncate alone leaves
     * a 16 MiB hole, and the file-backed cases then measure hole service
     * (zero-fill / shared-zero-page faults), not the cached-file reads
     * they exist to measure — bw_file_rd's "50x" row was exactly that
     * artifact (the reference kernel serves holes from memory too, Rux
     * re-resolved 512 extent lookups per read). Write the full size so
     * the file occupies real, cacheable blocks. */
    static char zeros[4096];
    for (size_t off = 0; off < SCRATCH_SZ; off += sizeof(zeros)) {
        if (write(fd, zeros, sizeof(zeros)) != (ssize_t)sizeof(zeros))
            break;
    }
    close(fd);
}

static pid_t start_server(const char *bench)
{
    char abs[192];
    snprintf(abs, sizeof(abs), "/test/%s", bench);
    char *sargv[3];
    sargv[0] = (char *)bench;
    sargv[1] = (char *)"-s";
    sargv[2] = NULL;
    pid_t sp = fork();
    if (sp == 0) {
        int nfd = open("/dev/null", O_WRONLY);
        if (nfd >= 0) { dup2(nfd, 1); dup2(nfd, 2); }
        execve(abs, sargv, NULL);
        _exit(127);
    }
    /* give the server a second to bind before the client connects */
    usleep(1000000);
    return sp;
}

static int read_spec(char *out, size_t cap)
{
    out[0] = 0;
    int cfd = open("/proc/cmdline", O_RDONLY);
    if (cfd < 0)
        return -1;
    char cl[1024];
    ssize_t n = read(cfd, cl, sizeof(cl) - 1);
    close(cfd);
    if (n <= 0)
        return -1;
    cl[n] = 0;
    char *p = strstr(cl, "lmbench=");
    if (!p)
        return -1;
    p += 8;
    char *e = p;
    while (*e && *e != ' ' && *e != '\n')
        e++;
    size_t len = (size_t)(e - p);
    if (len >= cap)
        len = cap - 1;
    memcpy(out, p, len);
    out[len] = 0;
    /* commas stand for spaces (the cmdline tokenizes on spaces) */
    for (size_t i = 0; i < len; i++)
        if (out[i] == ',')
            out[i] = ' ';
    return 0;
}

int main(void)
{
    chdir("/test");
    make_scratch();

    char spec[160];
    if (read_spec(spec, sizeof(spec)) != 0 || spec[0] == 0) {
        printf("LMBENCH NO-CASE (need lmbench=<bench> on the cmdline)\n");
        fflush(stdout);
        return 2;
    }

    char *bench = strtok(spec, " ");
    char *bargs[8];
    int bn = 0;
    char *tok = strtok(NULL, " ");
    while (tok && bn < 8) {
        bargs[bn++] = tok;
        tok = strtok(NULL, " ");
    }

    /* argv: <bench> -N <reps> [-W <warmup>] [bench args]
     * lmbench measures nothing without an explicit repetition count.
     * lat_connect's getopt has no -W, everyone else gets warmup 1. */
    char *real_argv[16];
    int argc2 = 0;
    real_argv[argc2++] = bench;
    real_argv[argc2++] = (char *)"-N";
    real_argv[argc2++] = (char *)"3";
    if (strcmp(bench, "lat_connect") != 0) {
        real_argv[argc2++] = (char *)"-W";
        real_argv[argc2++] = (char *)"1";
    }
    for (int k = 0; k < bn; k++)
        real_argv[argc2++] = bargs[k];
    real_argv[argc2] = NULL;

    printf("LMBENCH RUN %s\n", spec);
    fflush(stdout);

    pid_t server = is_net_bench(bench) ? start_server(bench) : -1;

    int pfd[2];
    if (pipe(pfd) != 0) {
        printf("LMBENCH PIPE-FAIL\n");
        return 3;
    }
    pid_t p = fork();
    if (p == 0) {
        close(pfd[0]);
        dup2(pfd[1], 1);
        dup2(pfd[1], 2);
        close(pfd[1]);
        {
            char abs[192];
            snprintf(abs, sizeof(abs), "/test/%s", bench);
            execve(abs, real_argv, NULL);
            printf("LMBENCH EXEC-FAIL %s\n", bench);
            fflush(stdout);
        }
        _exit(127);
    }
    close(pfd[1]);

    fcntl(pfd[0], F_SETFL, O_NONBLOCK);
    char buf[4096];
    ssize_t n;
    int ticks = 0;
    /* In-guest budget: 35 s default; `budget=<secs>` on the kernel
     * cmdline overrides it (perf lanes booting under host contention:
     * the auto-calibration of the slow cases needs more wall time than
     * 35 s of a contended vCPU, which used to turn a slow host into a
     * bogus TIMEOUT). The benchmark binaries and their arguments are
     * unchanged — this only decides when to kill. */
    int budget_ticks = 3500;
    {
        char cl[512];
        int cfd = open("/proc/cmdline", O_RDONLY);
        if (cfd >= 0) {
            ssize_t r = read(cfd, cl, sizeof(cl) - 1);
            close(cfd);
            if (r > 0) {
                cl[r] = 0;
                char *b = strstr(cl, "budget=");
                if (b) {
                    int secs = atoi(b + 7);
                    if (secs >= 5)
                        budget_ticks = secs * 100;
                }
            }
        }
    }
    for (;;) {
        n = read(pfd[0], buf, sizeof(buf));
        if (n > 0) {
            write(1, buf, (size_t)n);
            continue;
        }
        if (n == 0)
            break;
        int st0 = 0;
        if (waitpid(p, &st0, WNOHANG) == p) {
            while ((n = read(pfd[0], buf, sizeof(buf))) > 0)
                write(1, buf, (size_t)n);
            break;
        }
        if (++ticks >= budget_ticks) { /* default 35 s: enough for the
                                         * slow cases (lat_mem_rd 256M)
                                         * under KVM; real hangs still
                                         * hit the outer cap */
            kill(p, SIGTERM);
            usleep(200000);
            kill(p, SIGKILL);
            printf("LMBENCH TIMEOUT %s\n", bench);
            fflush(stdout);
            break;
        }
        usleep(10000);
    }
    close(pfd[0]);
    int st = 0;
    waitpid(p, &st, 0);
    if (server > 0) {
        kill(server, SIGKILL);
        waitpid(server, NULL, 0);
    }
    printf("LMBENCH DONE %s\n", bench);
    fflush(stdout);
    sync();
    /* end the boot immediately instead of idling to the outer cap */
    reboot(RB_POWER_OFF);
    return 0;
}
