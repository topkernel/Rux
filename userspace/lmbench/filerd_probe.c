/* filerd_probe.c — bw_file_rd profiling runner (PID 1).
 *
 * Same shape as the rig's lmbench_run, plus before/after snapshots of
 * /proc/meminfo (PageCacheOps hit/miss counters) and /proc/interrupts
 * (virtio-blk IRQ volume = disk I/O truth) so a warm-cache run can be
 * attributed without a kernel rebuild.
 *
 * Build: musl-gcc -static -O2 -o filerd_probe filerd_probe.c
 * Inject: debugfs -w -R "write filerd_probe /test/filerd_probe" <img>
 * Boot: init=/test/filerd_probe lmbench=bw_file_rd,1m,io_only,/tmp/lmbench.file
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <signal.h>
#include <sys/wait.h>

static void dump_file(const char *tag, const char *path)
{
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        printf("PROBE %s %s: open-fail\n", tag, path);
        return;
    }
    char buf[4096];
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0) {
        printf("PROBE %s %s: read-fail\n", tag, path);
        return;
    }
    buf[n] = 0;
    printf("==== PROBE %s %s ====\n", tag, path);
    fputs(buf, stdout);
    fflush(stdout);
}

int main(void)
{
    chdir("/test");

    /* 16 MiB scratch file, same as the rig runner. */
    {
        int fd = open("/tmp/lmbench.file", O_CREAT | O_TRUNC | O_WRONLY, 0644);
        if (fd >= 0) {
            char z[4096] = {0};
            for (int i = 0; i < 4096; i++)
                if (write(fd, z, sizeof(z)) != (ssize_t)sizeof(z))
                    break;
            close(fd);
        }
    }

    char spec[160];
    spec[0] = 0;
    int cfd = open("/proc/cmdline", O_RDONLY);
    if (cfd >= 0) {
        char cl[1024];
        ssize_t n = read(cfd, cl, sizeof(cl) - 1);
        close(cfd);
        if (n > 0) {
            cl[n] = 0;
            char *p = strstr(cl, "lmbench=");
            if (p) {
                p += 8;
                char *e = p;
                while (*e && *e != ' ' && *e != '\n') e++;
                size_t len = (size_t)(e - p) < sizeof(spec) - 1
                             ? (size_t)(e - p) : sizeof(spec) - 1;
                memcpy(spec, p, len);
                spec[len] = 0;
                for (size_t i = 0; i < len; i++)
                    if (spec[i] == ',') spec[i] = ' ';
            }
        }
    }
    if (!spec[0]) {
        printf("LMBENCH NO-CASE\n");
        return 2;
    }

    char *bench = strtok(spec, " ");
    char *bargs[8];
    int bn = 0;
    for (char *tok = strtok(NULL, " "); tok && bn < 8; tok = strtok(NULL, " "))
        bargs[bn++] = tok;

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

    /* Warm the file through the page cache once before the snapshot so
     * the BEFORE counters describe a fully cached file. */
    {
        int fd = open("/tmp/lmbench.file", O_RDONLY);
        if (fd >= 0) {
            char tmp[65536];
            while (read(fd, tmp, sizeof(tmp)) > 0) {}
            close(fd);
        }
    }

    printf("LMBENCH RUN %s (probe)\n", bench);
    fflush(stdout);

    dump_file("BEFORE", "/proc/meminfo");
    dump_file("BEFORE", "/proc/interrupts");
    dump_file("BEFORE", "/proc/filerd");

    int pfd[2];
    if (pipe(pfd) != 0)
        return 3;
    pid_t p = fork();
    if (p == 0) {
        close(pfd[0]);
        dup2(pfd[1], 1);
        dup2(pfd[1], 2);
        close(pfd[1]);
        char abs[192];
        snprintf(abs, sizeof(abs), "/test/%s", bench);
        execve(abs, real_argv, NULL);
        printf("LMBENCH EXEC-FAIL %s\n", bench);
        _exit(127);
    }
    close(pfd[1]);
    fcntl(pfd[0], F_SETFL, O_NONBLOCK);
    char buf[4096];
    ssize_t n;
    int ticks = 0;
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
        if (++ticks >= 5000) {
            kill(p, SIGTERM);
            usleep(200000);
            kill(p, SIGKILL);
            printf("LMBENCH TIMEOUT %s\n", bench);
            break;
        }
        usleep(10000);
    }
    int st = 0;
    waitpid(p, &st, 0);

    dump_file("AFTER", "/proc/meminfo");
    dump_file("AFTER", "/proc/interrupts");
    dump_file("AFTER", "/proc/filerd");

    printf("LMBENCH DONE\n");
    fflush(stdout);
    return 0;
}
