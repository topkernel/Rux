/*
 * jchurn.c — reproducer for a PRE-EXISTING VFS/SMP wedge (not the file
 * locks' fault: mode "none" uses no locking syscalls and still wedges).
 *
 * N processes loop { open(O_CREAT|O_RDWR), write, close, unlink } on a
 * shared file — exactly the sqlite rollback-journal churn pattern.
 * Observed on unmodified main (2026-09-30): after a few iterations the
 * watchdog reports
 *   DEADLOCK: spinlock stuck cpu=N lock=<VFS_MUTATION_LOCK> ra=<Spinlock::lock>
 * and the system never recovers, at -smp 1 AND -smp 2/4.
 *
 * GDB root cause (see task report): a task context-switches while
 * holding VFS_MUTATION_LOCK (its ti_preempt_count stays elevated — 2 —
 * after being switched out mid-critical-section), then ends up off the
 * run queue; the lock word stays 1 with no holder on any stack, and
 * every later open(O_CREAT)/unlink spins forever in
 * RawSpinlock::lock. Reproduces with or without flock/fcntl traffic.
 *
 * Build variants: -DDEFAULT_MODE='"none"' (default), '"flock"',
 * '"fcntl"' — all three wedge identically. Boots as init=/jchurn-*.
 */
#ifndef DEFAULT_MODE
#define DEFAULT_MODE "none"
#endif
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/wait.h>
#include <unistd.h>

#define PATH "/churn"

int main(int argc, char **argv)
{
    const char *mode = argc > 1 ? argv[1] : DEFAULT_MODE;
    int nproc = argc > 2 ? atoi(argv[2]) : 3;
    int iters = argc > 3 ? atoi(argv[3]) : 8;
    setvbuf(stdout, NULL, _IONBF, 0);

    for (int p = 0; p < nproc; p++) {
        pid_t pid = fork();
        if (pid == 0) {
            for (int i = 0; i < iters; i++) {
                int fd = open(PATH, O_RDWR | O_CREAT, 0644);
                if (fd < 0) { dprintf(2, "open errno=%d\n", errno); _exit(1); }
                if (write(fd, "x", 1) != 1) { dprintf(2, "write errno=%d\n", errno); _exit(2); }
                if (strcmp(mode, "flock") == 0 && flock(fd, LOCK_EX) != 0) {
                    dprintf(2, "flock errno=%d\n", errno); _exit(3);
                }
                if (strcmp(mode, "fcntl") == 0) {
                    struct flock fl = { F_WRLCK, SEEK_SET, 0, 0, 0 };
                    if (fcntl(fd, F_SETLK, &fl) != 0) {
                        dprintf(2, "fcntl errno=%d\n", errno); _exit(4);
                    }
                }
                usleep(2000);
                if (strcmp(mode, "flock") == 0) flock(fd, LOCK_UN);
                close(fd);
                if (unlink(PATH) != 0 && errno != ENOENT) {
                    dprintf(2, "unlink errno=%d\n", errno); _exit(5);
                }
            }
            _exit(0);
        }
    }
    int fails = 0;
    for (int p = 0; p < nproc; p++) {
        int st;
        pid_t w = wait(&st);
        if (w < 0 || !WIFEXITED(st) || WEXITSTATUS(st) != 0) fails++;
    }
    printf("CHURN-%s-DONE fail=%d\n", mode, fails);
    if (getpid() == 1) for (;;) sleep(60);
    return fails ? 1 : 0;
}
