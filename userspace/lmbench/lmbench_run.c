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

    /* argv: <bench> -N <reps> -W <warmup> [bench args]
     * lmbench measures nothing without an explicit repetition count. */
    char *real_argv[16];
    int argc2 = 0;
    real_argv[argc2++] = bench;
    real_argv[argc2++] = (char *)"-N";
    real_argv[argc2++] = (char *)"3";
    real_argv[argc2++] = (char *)"-W";
    real_argv[argc2++] = (char *)"1";
    for (int k = 0; k < bn; k++)
        real_argv[argc2++] = bargs[k];
    real_argv[argc2] = NULL;

    printf("LMBENCH RUN %s\n", spec);
    fflush(stdout);

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
        if (++ticks >= 2000) {   /* 20 s */
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
    printf("LMBENCH DONE %s\n", bench);
    fflush(stdout);
    sync();
    return 0;
}
