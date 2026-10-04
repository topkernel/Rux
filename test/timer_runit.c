/* runit.c — static PID-1 for Rux/Ubuntu rootfs testing.
 *
 * Reads /root/auto.sh; every non-blank, non-# line is fork/exec'd
 * (words split on spaces; env is fixed). Exit status is printed as
 * [st=N] on stdout. Lines of the form "SLEEP N" pause N seconds.
 * After the script ends, the process sleeps forever (keeps PID 1 alive).
 */
#include <unistd.h>
#include <fcntl.h>
#include <sys/wait.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <time.h>

static char buf[8192];

static void run(char *line)
{
    char *av[64];
    int n = 0;
    char *p = line;
    static char *ev[] = {
        "HOME=/root", "PATH=/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "TERM=linux", "DISPLAY=:0", "LANG=C", "XAUTHORITY=/root/.Xauthority",
        NULL
    };
    while (*p && n < 62) {
        while (*p == ' ') p++;
        if (!*p) break;
        av[n++] = p;
        while (*p && *p != ' ') p++;
        if (*p) *p++ = 0;
    }
    av[n] = NULL;
    if (!n) return;
    if (!strcmp(av[0], "SLEEP")) {
        int s = av[1] ? atoi(av[1]) : 1;
        struct timespec ts = { s, 0 };
        printf("[sleep %d]\n", s); fflush(stdout);
        nanosleep(&ts, NULL);
        return;
    }

    int bg = 0;
    if (!strcmp(av[0], "BG")) {
        bg = 1;
        for (int i = 0; av[i + 1]; i++) av[i] = av[i + 1];
        av[n - 1] = NULL; /* the shift leaves a duplicate tail entry */
        if (!av[0]) return;
    }

    pid_t pid = fork();
    if (pid == 0) {
        execve(av[0], av, ev);
        printf("[exec %s failed]\n", av[0]); fflush(stdout);
        _exit(127);
    }
    if (bg) {
        printf("[bg pid=%d]\n", pid); fflush(stdout);
        return;
    }
    int st = 0;
    waitpid(pid, &st, 0);
    printf("[st=%d]\n", st); fflush(stdout);
}

int main(void)
{
    int fd = open("/root/auto.sh", O_RDONLY);
    if (fd < 0) {
        printf("runit: no /root/auto.sh\n"); fflush(stdout);
    } else {
        ssize_t r = read(fd, buf, sizeof(buf) - 1);
        if (r <= 0) {
            printf("runit: empty script\n"); fflush(stdout);
        } else {
            buf[r] = 0;
            char *save = NULL;
            for (char *l = strtok_r(buf, "\n", &save); l;
                 l = strtok_r(NULL, "\n", &save)) {
                if (l[0] == '#' || !l[0]) continue;
                printf(">> %s\n", l); fflush(stdout);
                run(l);
            }
        }
        close(fd);
    }
    printf("runit: script done, idling\n"); fflush(stdout);
    for (;;) {
        int st;
        pause();
        while (waitpid(-1, &st, WNOHANG) > 0)
            ;
    }
    return 0;
}
