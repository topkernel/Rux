/* geekbench_run.c — Geekbench runner for the Rux rig (PID 1).
 *
 * Same shape as the lmbench runner (userspace/lmbench/lmbench_run.c):
 * the case comes from the kernel cmdline, the child's output is
 * forwarded through a pipe, and the run is bounded by a timeout.
 * Geekbench runs 10+ minutes, so the default timeout is 900 s,
 * overridable with gtimeout=<seconds> on the cmdline.
 *
 * Cmdline parameters:
 *   geekbench=<mode>   mode: cpu (default) — maps to `geekbench6 --cpu`
 *   gtimeout=<sec>     in-guest kill timeout (default 900)
 *
 * Markers for the boot driver:
 *   GEEKBENCH RUN / GEEKBENCH DONE / GEEKBENCH TIMEOUT / GEEKBENCH EXEC-FAIL
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <signal.h>
#include <errno.h>
#include <netdb.h>
#include <sys/stat.h>
#include <sys/select.h>
#include <sys/wait.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <net/if.h>
#include <net/route.h>
#include <arpa/inet.h>

/* Bring eth0 up with the QEMU user-network addressing so the Geekbench
 * result upload (HTTPS to browser.geekbench.com) has a chance to work;
 * without it the run still completes but no score table is retrievable
 * (Geekbench 5 only prints scores into the uploaded browser result). */
static void net_setup(void)
{
    struct ifreq ifr;
    memset(&ifr, 0, sizeof(ifr));
    strcpy(ifr.ifr_name, "eth0");

    int s = socket(AF_INET, SOCK_DGRAM, 0);
    if (s < 0) {
        printf("GEEKBENCH NET no-socket\n");
        return;
    }

    struct sockaddr_in *sa = (struct sockaddr_in *)&ifr.ifr_addr;
    sa->sin_family = AF_INET;

    inet_pton(AF_INET, "10.0.2.15", &sa->sin_addr);
    if (ioctl(s, SIOCSIFADDR, &ifr) != 0)
        printf("GEEKBENCH NET setaddr-fail errno=%d\n", errno);

    inet_pton(AF_INET, "255.255.255.0", &sa->sin_addr);
    if (ioctl(s, SIOCSIFNETMASK, &ifr) != 0)
        printf("GEEKBENCH NET setmask-fail errno=%d\n", errno);

    short flags = IFF_UP | IFF_RUNNING | IFF_BROADCAST;
    memcpy(&ifr.ifr_flags, &flags, sizeof(flags));
    if (ioctl(s, SIOCSIFFLAGS, &ifr) != 0)
        printf("GEEKBENCH NET ifup-fail errno=%d\n", errno);

    /* default route via 10.0.2.2 */
    struct rtentry rt;
    memset(&rt, 0, sizeof(rt));
    struct sockaddr_in dst, gw, mask;
    memset(&dst, 0, sizeof(dst)); dst.sin_family = AF_INET; dst.sin_addr.s_addr = 0;
    memset(&gw, 0, sizeof(gw));   gw.sin_family = AF_INET;  inet_pton(AF_INET, "10.0.2.2", &gw.sin_addr);
    memset(&mask, 0, sizeof(mask)); mask.sin_family = AF_INET; mask.sin_addr.s_addr = 0;
    memcpy(&rt.rt_dst, &dst, sizeof(dst));
    memcpy(&rt.rt_gateway, &gw, sizeof(gw));
    memcpy(&rt.rt_genmask, &mask, sizeof(mask));
    rt.rt_flags = RTF_UP | RTF_GATEWAY;
    rt.rt_dev = "eth0";
    if (ioctl(s, SIOCADDRT, &rt) != 0)
        printf("GEEKBENCH NET addroute-fail errno=%d\n", errno);

    close(s);

    FILE *f = fopen("/etc/resolv.conf", "w");
    if (f) {
        fprintf(f, "nameserver 10.0.2.3\n");
        fclose(f);
        printf("GEEKBENCH NET up (10.0.2.15/24 gw 10.0.2.2 dns 10.0.2.3)\n");
    } else {
        printf("GEEKBENCH NET resolv-write-fail\n");
    }
    fflush(stdout);
}

static void read_param(const char *key, char *out, size_t cap)
{
    out[0] = 0;
    int cfd = open("/proc/cmdline", O_RDONLY);
    if (cfd < 0)
        return;
    char cl[1024];
    ssize_t n = read(cfd, cl, sizeof(cl) - 1);
    close(cfd);
    if (n <= 0)
        return;
    cl[n] = 0;
    size_t klen = strlen(key);
    char *p = cl;
    while ((p = strstr(p, key)) != NULL) {
        /* must start a token */
        if (p == cl || p[-1] == ' ')
            break;
        p += klen;
    }
    if (!p)
        return;
    p += klen;
    char *e = p;
    while (*e && *e != ' ' && *e != '\n')
        e++;
    size_t len = (size_t)(e - p);
    if (len >= cap)
        len = cap - 1;
    memcpy(out, p, len);
    out[len] = 0;
}

int main(void)
{
    mkdir("/test", 0755);
    chdir("/test/geekbench");

    net_setup();

    char mode[32];
    read_param("geekbench=", mode, sizeof(mode));
    if (mode[0] == 0 || strcmp(mode, "cpu") == 0)
        snprintf(mode, sizeof(mode), "cpu");

    char tbuf[16];
    read_param("gtimeout=", tbuf, sizeof(tbuf));
    long timeout_s = 900;
    if (tbuf[0] != 0) {
        long v = strtol(tbuf, NULL, 10);
        if (v > 0)
            timeout_s = v;
    }

    char *real_argv[8];
    int argc2 = 0;
    if (strcmp(mode, "netprobe") == 0) {
        /* Diagnostic: resolve + connect the upload path, then exit. */
        struct addrinfo hints, *res = NULL;
        memset(&hints, 0, sizeof(hints));
        hints.ai_family = AF_INET;
        hints.ai_socktype = SOCK_STREAM;
        int rc = getaddrinfo("browser.geekbench.com", "443", &hints, &res);
        printf("GEEKBENCH NETPROBE resolve rc=%d\n", rc);
        if (rc == 0 && res) {
            char addr[48] = "?";
            getnameinfo(res->ai_addr, res->ai_addrlen, addr, sizeof(addr), NULL, 0,
                        NI_NUMERICHOST);
            int cfd = socket(AF_INET, SOCK_STREAM, 0);
            fcntl(cfd, F_SETFL, O_NONBLOCK);
            int cr = connect(cfd, res->ai_addr, res->ai_addrlen);
            printf("GEEKBENCH NETPROBE connect(%s:443) rc=%d errno=%d\n", addr, cr, errno);
            if (cr != 0 && (errno == EINPROGRESS)) {
                fd_set wf;
                struct timeval tv = { 12, 0 };
                FD_ZERO(&wf);
                FD_SET(cfd, &wf);
                int sr = select(cfd + 1, NULL, &wf, NULL, &tv);
                int soerr = 0;
                socklen_t slen = sizeof(soerr);
                getsockopt(cfd, SOL_SOCKET, SO_ERROR, &soerr, &slen);
                printf("GEEKBENCH NETPROBE select=%d soerr=%d\n", sr, soerr);
            }
            close(cfd);
        }
        printf("GEEKBENCH DONE netprobe\n");
        fflush(stdout);
        sync();
        return 0;
    }
    real_argv[argc2++] = (char *)"./geekbench5";
    if (strcmp(mode, "cpu") == 0)
        real_argv[argc2++] = (char *)"--cpu";
    else if (strcmp(mode, "sysinfo") == 0)
        real_argv[argc2++] = (char *)"--sysinfo";
    real_argv[argc2] = NULL;

    char *envp[] = {
        (char *)"HOME=/root",
        (char *)"TMPDIR=/tmp",
        (char *)"PATH=/usr/local/bin:/usr/bin:/bin",
        NULL,
    };

    printf("GEEKBENCH RUN %s (timeout %ld s)\n", mode, timeout_s);
    fflush(stdout);

    int pfd[2];
    if (pipe(pfd) != 0) {
        printf("GEEKBENCH PIPE-FAIL\n");
        return 3;
    }
    pid_t p = fork();
    if (p == 0) {
        close(pfd[0]);
        dup2(pfd[1], 1);
        dup2(pfd[1], 2);
        close(pfd[1]);
        setpgid(0, 0);
        execve("/test/geekbench/geekbench5", real_argv, envp);
        printf("GEEKBENCH EXEC-FAIL ./geekbench5\n");
        fflush(stdout);
        _exit(127);
    }
    close(pfd[1]);
    setpgid(p, p); /* parent-side too: kill the whole group on timeout */

    fcntl(pfd[0], F_SETFL, O_NONBLOCK);
    char buf[4096];
    ssize_t n;
    long ticks = 0;
    const long ticks_max = timeout_s * 100; /* 10 ms per tick */
    for (;;) {
        n = read(pfd[0], buf, sizeof(buf));
        if (n > 0) {
            write(1, buf, (size_t)n);
            ticks = 0; /* progress resets the idle clock, not the cap */
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
        if (++ticks >= ticks_max) {
            kill(-p, SIGTERM);
            usleep(300000);
            kill(-p, SIGKILL);
            printf("GEEKBENCH TIMEOUT %s after %ld s\n", mode, timeout_s);
            fflush(stdout);
            break;
        }
        usleep(10000);
    }
    close(pfd[0]);
    int st = 0;
    waitpid(p, &st, 0);
    if (WIFEXITED(st))
        printf("GEEKBENCH EXIT status=%d\n", WEXITSTATUS(st));
    else if (WIFSIGNALED(st))
        printf("GEEKBENCH EXIT signal=%d\n", WTERMSIG(st));
    printf("GEEKBENCH DONE %s\n", mode);
    fflush(stdout);
    sync();
    return 0;
}
