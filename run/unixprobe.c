/* unixprobe — AF_UNIX stream data-integrity probe (the X-protocol shape).
 *
 * Mutter sends render requests to the X server over an AF_UNIX stream
 * socket: thousands of writes of a few KB, interleaved with reads. If
 * the socket path drops, duplicates, or zeroes data, the server never
 * completes a valid request sequence and NOTHING is painted — the
 * black-screen face with both sides healthy.
 *
 * Parent = "client" (writes requests), child = "server" (reads, echoes
 * a digest). Checks: byte-exact length + FNV-1a checksum per message,
 * both directions, 2000 messages of 8 KiB.
 *
 * Exit code = number of failures.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

static unsigned long fnv1a(const unsigned char *b, size_t n)
{
    unsigned long h = 1469598103934665603UL;
    for (size_t i = 0; i < n; i++) {
        h ^= b[i];
        h *= 1099511628211UL;
    }
    return h;
}

#define MSGSZ 8192
#define NMSG 2000

static int readn(int fd, void *buf, size_t n)
{
    size_t got = 0;
    while (got < n) {
        ssize_t r = read(fd, (char *)buf + got, n - got);
        if (r < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (r == 0) return -2;
        got += r;
    }
    return 0;
}

static int writen(int fd, const void *buf, size_t n)
{
    size_t put = 0;
    while (put < n) {
        ssize_t w = write(fd, (const char *)buf + put, n - put);
        if (w < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        put += w;
    }
    return 0;
}

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);

    /* filesystem pathname socket, like the X server's /tmp/.X11-unix/X0 */
    const char *path = "/tmp/.unixprobe.sock";
    unlink(path);
    int ls = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un sa = { .sun_family = AF_UNIX };
    strncpy(sa.sun_path, path, sizeof sa.sun_path - 1);
    if (bind(ls, (struct sockaddr *)&sa, sizeof sa) < 0 || listen(ls, 1) < 0) {
        printf("UNIXP bind/listen failed: %s\n", strerror(errno));
        return 9;
    }

    pid_t c = fork();
    if (c == 0) {
        /* server: accept, then for each message read + echo digest */
        int cs = accept(ls, NULL, NULL);
        if (cs < 0) _exit(1);
        close(ls);
        static unsigned char buf[MSGSZ];
        int bad = 0;
        for (int i = 0; i < NMSG; i++) {
            if (readn(cs, buf, MSGSZ) != 0) { bad += 100; break; }
            unsigned long d = fnv1a(buf, MSGSZ);
            if (writen(cs, &d, sizeof d) != 0) { bad += 200; break; }
        }
        close(cs);
        _exit(bad ? 2 : 0);
    }

    /* client: fresh socket, connect to the same path (the fork
       duplicated the listener; the child accepts on it) */
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    int retries = 0;
    while (connect(fd, (struct sockaddr *)&sa, sizeof sa) < 0 && retries < 100) {
        usleep(2000);
        retries++;
    }
    if (retries >= 100) {
        printf("UNIXP connect failed: %s\n", strerror(errno));
        return 9;
    }
    static unsigned char buf[MSGSZ];
    int bad = 0;
    for (int i = 0; i < NMSG; i++) {
        for (int j = 0; j < MSGSZ; j++) buf[j] = (unsigned char)(i + j);
        if (writen(fd, buf, MSGSZ) != 0) { bad += 300; break; }
        unsigned long d = 0;
        if (readn(fd, &d, sizeof d) != 0) { bad += 400; break; }
        if (d != fnv1a(buf, MSGSZ)) {
            bad++;
            if (bad < 5) printf("UNIXP MISMATCH msg=%d digest=%#lx\n", i, d);
        }
        if (i % 500 == 499) printf("UNIXP progress %d/%d bad=%d\n", i + 1, NMSG, bad);
    }
    int st = 0;
    waitpid(c, &st, 0);
    if (!(WIFEXITED(st) && WEXITSTATUS(st) == 0)) {
        printf("UNIXP server child failed st=%#x\n", st);
        bad += 1000;
    }
    printf("UNIXP-DONE fails=%d\n", bad);
    return bad ? 1 : 0;
}
