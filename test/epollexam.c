/*
 * epollexam.c — epoll_pwait argument-matrix probe (fsfuzz wedge chase):
 * calls epoll_pwait/epoll_wait with the exact fsfuzz-parked shape and
 * neighbors, each with a bounded timeout, printing the errno of each.
 * Any call that does NOT return within its timeout window = the park;
 * the kernel should answer EINVAL/EBADF/EFAULT for every non-epoll fd.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/syscall.h>
#include <unistd.h>

static char evbuf[64] __attribute__((aligned(4096)));

static int one(const char *what, long nr, long a, long b, long c, long d,
               long e, long f)
{
    printf("EEXAM %s nr=%ld a=%#lx b=%#lx c=%#lx d=%#lx e=%#lx ...",
           what, nr, a, b, c, d, e);
    fflush(stdout);
    long r = syscall(nr, a, b, c, d, e, f);
    printf(" -> %ld errno=%d\n", r, errno);
    fflush(stdout);
    return (int)r;
}

int main(void)
{
    printf("EEXAM-START pid=%d\n", getpid());
    fflush(stdout);

    /* the exact fsfuzz-parked shape: epfd=0 (console), events=0x1000
     * (unmapped), maxevents/timeout from the fuzz pools */
    one("pwait fd0 unmapped-ev", 22, 0, 0x1000, 1, -1, 0, 0);
    one("pwait fd0 unmapped-ev t0", 22, 0, 0x1000, 1, 0, 0, 0);
    one("pwait fd1 unmapped-ev", 22, 1, 0x1000, 1, -1, 0, 0);
    one("pwait fd2 unmapped-ev", 22, 2, 0x1000, 1, 100, 0, 0);
    one("pwait fd3 unmapped-ev", 22, 3, 0x1000, 1, 100, 0, 0);
    one("pwait fd7 unmapped-ev", 22, 7, 0x1000, 1, 100, 0, 0);
    one("pwait fd100 unmapped-ev", 22, 100, 0x1000, 1, 100, 0, 0);
    one("pwait fd0 mapped-ev", 22, 0, (long)evbuf, 1, 100, 0, 0);
    one("wait  fd0 mapped-ev", 22 - 0, 0, (long)evbuf, 1, 100, 0, 0);
    one("wait2 fd0 mapped-ev", 21, 0, (long)evbuf, 1, 100, 0, 0);
    one("pwait fd0 sigmask-bad", 22, 0, (long)evbuf, 1, 100, 0x7ffffffff000UL, 8);
    one("pwait fd0 sigsize-bad", 22, 0, (long)evbuf, 1, 100, (long)evbuf, 4);
    one("pwait fd0 maxev0", 22, 0, (long)evbuf, 0, 100, 0, 0);
    one("pwait fd0 maxev-bad", 22, 0, (long)evbuf, 0x8000, 100, 0, 0);

    printf("EEXAM-DONE\n");
    fflush(stdout);
    /* stay alive so the serial keeps flushing and the round classifies */
    for (;;) { pause(); }
    return 0;
}
