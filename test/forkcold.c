// forkcold.c — minimal fork + cold-child-page probe: the child calls a
// function living on a text page the parent never executed (noclone,
// noinline, called only by the child), then writes an anon mapping.
// Reproduces the riscv64 mmapfix MF8 child SIGILL (epc page contained
// file offset 0 content) without any MAP_FIXED involvement.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#define PAGE 4096UL

__attribute__((noinline, section(".text.coldprobe"))) static int
cold_child_fn(int acc)
{
    /* Enough straight-line code to span past one page boundary. */
    for (volatile int i = 0; i < 200000; i++)
        acc = acc * 3 + i;
    return acc & 0x7f;
}

static void raw(const char *s, unsigned long v)
{
    char buf[64];
    int n = snprintf(buf, sizeof(buf), "%s%lx\n", s, v);
    write(1, buf, n);
}

int main(void)
{
    unsigned char *an = mmap(NULL, 4 * PAGE, PROT_READ | PROT_WRITE,
                             MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (an == MAP_FAILED) {
        raw("FC-FAIL mmap errno=", errno);
        return 1;
    }
    pid_t pid = fork();
    if (pid == 0) {
        an[0x1000] = 0x5a;
        int r = cold_child_fn(7);
        raw("FC child cold=", (unsigned long)r);
        raw("FC child byte=", an[0x1000]);
        _exit(r == cold_child_fn(9) ? 1 : 0);
    }
    int st = 0;
    waitpid(pid, &st, 0);
    if (!WIFEXITED(st) || WEXITSTATUS(st) != 0) {
        raw("FC-FAIL child st=", (unsigned long)st);
        return 1;
    }
    if (an[0x1000] != 0) {
        raw("FC-FAIL leak=", an[0x1000]);
        return 1;
    }
    write(1, "FC-PASS\n", 8);
    return 0;
}
