// spawncrash.c — controlled repro for the gsd-helper child crash family.
//
// Symptom under the session storm: gsd-spawned /bin/sh helpers die with
// SEGV epc=0x2aaaaae2b6 addr=0x8 (a relocated data slot dereferenced as
// a near-NULL pointer), always the same epc, several times per boot.
// All those helpers are created through glib -> posix_spawn, which uses
// clone(CLONE_VM|CLONE_VFORK); the plain fork()+exec path was never
// observed to crash. This probe runs both paths in controlled loops and
// reports per-path crash counts, so a kernel-side CLONE_VM/VFORK defect
// shows as path A failing while path B stays clean.
//
// Also arms path C: posix_spawn of "/bin/sh -c pwd" (dash startup runs
// getcwd — exercises the getcwd ABI fix end to end).
//
// Build: against the image sysroot (glibc 2.35), dynamic.
// Run: prints PROGRESS lines every 25 children and a SUMMARY at exit.
#include <errno.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

static int run_loop(const char *tag, int use_posix_spawn,
                    const char *file, char *const argv[]) {
    int crashes = 0, ok = 0, other = 0;
    for (int i = 0; i < 100; i++) {
        pid_t pid;
        int st;
        if (use_posix_spawn) {
            int rc = posix_spawn(&pid, file, NULL, NULL, argv, environ);
            if (rc != 0) {
                printf("[%s] spawn rc=%d errno=%d (%s)\n", tag, rc, errno,
                       strerror(errno));
                other++;
                continue;
            }
        } else {
            pid = fork();
            if (pid == 0) {
                execve(file, argv, environ);
                _exit(127);
            }
            if (pid < 0) {
                printf("[%s] fork errno=%d\n", tag, errno);
                other++;
                continue;
            }
        }
        if (waitpid(pid, &st, 0) < 0) {
            printf("[%s] waitpid errno=%d\n", tag, errno);
            other++;
            continue;
        }
        if (WIFSIGNALED(st) && (WTERMSIG(st) == SIGSEGV ||
                                WTERMSIG(st) == SIGBUS ||
                                WTERMSIG(st) == SIGILL)) {
            crashes++;
            printf("[%s] child %d CRASHED sig=%d\n", tag, pid,
                   WTERMSIG(st));
        } else if (WIFEXITED(st) && WEXITSTATUS(st) == 0) {
            ok++;
        } else {
            other++;
            printf("[%s] child st=0x%x\n", tag, st);
        }
        if ((i % 25) == 24) {
            printf("[PROGRESS] %s %d done: ok=%d crash=%d other=%d\n",
                   tag, i + 1, ok, crashes, other);
            fflush(stdout);
        }
    }
    printf("[SUMMARY] %s: ok=%d crash=%d other=%d\n", tag, ok, crashes,
           other);
    fflush(stdout);
    return crashes;
}

int main(void) {
    char *const true_argv[] = { "/bin/sh", "-c", "exit 0", NULL };
    char *const pwd_argv[] = { "/bin/sh", "-c", "pwd", NULL };

    printf("== spawncrash start ==\n");
    fflush(stdout);

    run_loop("A-posixspawn-sh-true", 1, "/bin/sh", true_argv);
    run_loop("B-forkexec-sh-true", 0, "/bin/sh", true_argv);
    run_loop("C-posixspawn-sh-pwd", 1, "/bin/sh", pwd_argv);
    run_loop("D-forkexec-sh-pwd", 0, "/bin/sh", pwd_argv);

    printf("== spawncrash done ==\n");
    fflush(stdout);
    return 0;
}
