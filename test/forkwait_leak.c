/* forkwait_leak.c — minimal kernel-heap leak repro for the s11 fsfuzz
 * catch (2026-10-27-s11-heapexhaustion-fsfuzz.txt).
 *
 * ROOT CAUSE (s12, confirmed and fixed): leg 2. sys_pipe2 allocates the
 * 64KB+1 PipeBuffer ring inside create_pipe() BEFORE any fd work; the
 * whole Arc<Pipe> ownership lives as two raw pointers in the end Files'
 * private_data, consumed only by the PIPE_OPS close op. File::drop runs
 * that op solely once close_fd committed (close_pending), so every
 * error return between create_pipe() and install (alloc_fd EMFILE x2,
 * fdtable gone, install_fd failure) dropped the Arc<File>s without the
 * op and leaked both raw refs plus the ring — one full 128KB buddy
 * block (65537B rounds up) per failed call. Fix: abort_pipe_creation()
 * in fs/pipe.rs runs the close op on uninstalled ends (kernel/src/
 * syscall/io.rs sys_pipe2). Before/after on this probe: ring site
 * live=100 a=100 f=0 -> live=0 a=100 (MEMLIVE, every dump).
 *
 * Leg 1 (fork+wait churn) was the OTHER fingerprint member (Task
 * 11064B / SignalStruct 2088B, "live" ~97% of forks) and is now
 * adjudicated a memwatch ACCOUNTING ARTIFACT, not a leak: packed
 * sub-page blocks share a PAGE_SITE mark, so a neighbor's free clears
 * the mark and the next free's attribution is dropped. Counter boot
 * (fwl-cnt, s11): release_task 3000/3000, freeing task_put 3000/3000,
 * free_task_slot dealloc 3000/3000 — every child Task is really freed;
 * page-aligned sites attribute exactly (stack 131072B: f=236 + 64
 * baseline live = a=300, perfect arithmetic). The leg stays in the
 * probe as the standing watch for a real task-leak regression.
 *
 * After both legs the probe parks 80s so the dfx=periodic (30s) dumps
 * capture the MEMLIVE table; the driver greps the alloc_task_slot /
 * SignalStruct / PipeBuffer site rows and compares live counts.
 * PASS = ring site live==0; Task/Signal live expected steady-state
 * (artifact level, ~2000 for nforks=3000).
 *
 * Build: run/fleet/bin/riscv64-musl-gcc -static -O1 -o forkwait_leak forkwait_leak.c
 * Boot:  init=/test/forkwait_leak dfx=periodic,memwatch,watchdog
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>

#define NFORKS 3000
#define NPIPES 100

int main(int argc, char **argv)
{
    int nforks = (argc > 1) ? atoi(argv[1]) : NFORKS;
    setvbuf(stdout, NULL, _IOLBF, 0);
    printf("FWL-START pid=%d nforks=%d\n", getpid(), nforks);

    /* Leg 1: fork+wait churn */
    for (int i = 0; i < nforks; i++) {
        pid_t pid = fork();
        if (pid == 0)
            _exit(0);
        if (pid < 0) {
            printf("FWL-ODD fork errno=%d at %d\n", errno, i);
            break;
        }
        if (waitpid(pid, NULL, 0) != pid)
            printf("FWL-ODD waitpid errno=%d at %d\n", errno, i);
    }
    printf("FWL-FORKS-DONE n=%d\n", nforks);

    /* Leg 2: pipe() storm against a full fd table */
    int filled = 0;
    for (;;) {
        int fd = dup(0);
        if (fd < 0)
            break;
        filled++;
        if (filled >= 4096)
            break;
    }
    printf("FWL-FDTABLE-FULL filled=%d\n", filled);
    int emfile = 0, other = 0;
    for (int i = 0; i < NPIPES; i++) {
        int pfd[2];
        if (pipe(pfd) < 0) {
            if (errno == EMFILE)
                emfile++;
            else {
                other++;
                printf("FWL-ODD pipe errno=%d at %d\n", errno, i);
            }
            continue;
        }
        close(pfd[0]);
        close(pfd[1]);
    }
    printf("FWL-PIPES-DONE emfile=%d other=%d\n", emfile, other);

    /* Park for periodic MEMLIVE captures (dfx=periodic is 30s cadence) */
    for (int s = 0; s < 80; s++) {
        if ((s % 15) == 0)
            printf("FWL-PARK %d\n", s);
        sleep(1);
    }
    printf("FWL-PASS\n");
    return 0;
}
