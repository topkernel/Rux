/*
 * execloop.c — pure-exec leak probe.
 *
 * Usage: execloop <iters> [prog]
 *
 * Runs <iters> rounds of fork + execve(prog, ...) + waitpid, printing a
 * HEAPMARK line with HeapUsed from /proc/meminfo every 100 rounds. With
 * no <prog>, the child execs this binary with "-1" so it exits at once
 * (still a full ELF load + auxv/env setup).
 *
 * Build: riscv64-linux-gnu-gcc -static -O2 -o execloop execloop.c
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/wait.h>
#include <fcntl.h>

extern char **environ;

static long read_heap_used(void)
{
    int fd = open("/proc/meminfo", O_RDONLY);
    if (fd < 0)
        return -1;
    char buf[4096];
    int n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n <= 0)
        return -1;
    buf[n] = 0;
    const char *p = strstr(buf, "HeapUsed:");
    if (!p)
        return -1;
    return strtol(p + 9, NULL, 10);
}

static const char *self;

static void one_exec(const char *prog)
{
    pid_t pid = fork();
    if (pid < 0) {
        dprintf(2, "fork errno\n");
        exit(1);
    }
    if (pid == 0) {
        char *av[3];
        av[0] = (char *)prog;
        av[1] = (char *)"-1";
        av[2] = NULL;
        execve(prog, av, environ);
        _exit(127);
    }
    int st;
    while (waitpid(pid, &st, 0) < 0)
        ;
}

int main(int argc, char **argv, char **envp)
{
    environ = envp;
    self = argv[0];
    int iters = argc > 1 ? atoi(argv[1]) : 100;
    const char *prog = argc > 2 ? argv[2] : self;

    if (iters < 0) /* child of one_exec: exec'd here, exit immediately */
        return 0;

    setvbuf(stdout, NULL, _IONBF, 0);
    printf("EXECLOOP start iters=%d prog=%s heap=%ld kB\n", iters, prog,
           read_heap_used());
    for (int i = 1; i <= iters; i++) {
        one_exec(prog);
        if (i % 100 == 0)
            printf("HEAPMARK %d %ld\n", i, read_heap_used());
    }
    printf("EXECLOOP done heap=%ld kB\n", read_heap_used());
    return 0;
}
