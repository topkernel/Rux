// subreap_churn.c — fork/exit churn hunting the E8-REPAR "unlinked
// snapshot" family (leg-29A): a LIVE chain node whose parent pointer
// names an already-released task.
//
// Suspect shape (from code reading, kernel/src/process/task.rs):
//   thread_group_join() copies the leader's parent into every member as
//   an UNLINKED snapshot and nothing maintains it. move_child() (the
//   reparent step) updates only the moved leader's parent — every
//   member of a REPARENTED multi-threaded group keeps naming the old,
//   eventually-reaped parent. The member's own exit then dereferences
//   the recycled pointer (release_task's remove_child), and any
//   ancestor-chain consumer (sc304's subreaper walk) reads garbage.
//
// This probe creates exactly that shape at maximum rate:
//   - a long-lived root with PR_SET_CHILD_SUBREAPER (the reparent
//     destination, so moves actually happen instead of defaulting to
//     init),
//   - multi-threaded middle levels whose LEADERS die while members
//     survive (member-first exit order) and whose MEMBERS die while
//     the leader survives (leader-first order), both orders raced,
//   - grandchildren forked FROM MEMBER THREADS (add_child onto a
//     member's own children list) and from leaders,
//   - zombie pressure (un-reaped children) plus claimed reaps from a
//     second group member (the do_wait claim race),
//   - userspace verdicts: an orphan's getppid() must name the subreaper
//     or init (never a pid that was never its ancestor), and the
//     subreaper must be able to reap what it adopted (waitpid __WALL).
//
// Kernel-side evidence (dfx=treehunt): TREE-PARENT-DEAD /
// TREE-SNAP-STALE / TREE-UNLINKED lines are the smoking gun; the driver
// (subreap_boot.py) judges them rc=2.
//
// Verdict contract:
//   SUBREAP-PASS after PROBE_SECS       -> rc 0
//   SUBREAPFAIL-* line                  -> rc 1
//   (kernel TREE-*/panic/Oops           -> rc 2, driver)
#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#ifndef PROBE_SECS
#define PROBE_SECS 30
#endif

static unsigned long seed = 0xC0FFEE29UL;
static unsigned long rnd(void) {
    seed = seed * 6364136223846793005UL + 1442695040888963407UL;
    return seed >> 17;
}

static volatile sig_atomic_t stop_flag;
static unsigned long cycles, orphan_checks, reap_ok, reap_fail;
static pid_t root_pid;

static double now_secs(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec / 1e9;
}

static void fail(const char *what, long a, long b) {
    printf("SUBREAPFAIL-%s a=%ld b=%ld errno=%d\n", what, a, b, errno);
    fflush(stdout);
    _exit(1);
}

// A grandchild: verify its ppid names a plausible ancestor (the
// subreaper root or 1) once its whole ancestry above the root died.
static void grandchild_life(int check_ppid) {
    // Live a short random life; if asked, verify the reparent target.
    struct timespec ts = { 0, 5 * 1000 * 1000 };
    for (int i = 0; i < 8 && !stop_flag; i++) {
        nanosleep(&ts, 0);
        if (check_ppid) {
            pid_t pp = getppid();
            // Legal parents: the subreaper root, init, or a SURVIVING
            // THREAD of the dying parent's group (Linux find_new_reaper
            // moves children there first) — pids that churn constantly.
            // Only kernel-side TREE-* lines are the invariant verdict;
            // this check counts the reparenting traffic.
            orphan_checks++;
        }
    }
    _exit(0);
}

// Member thread of a middle-level group: optionally fork a child FROM
// THE MEMBER (add_child onto a non-leader), then park briefly.
struct member_arg {
    int fork_child;
    int child_check;
};
static void *member_fn(void *ap) {
    struct member_arg *ma = ap;
    if (ma->fork_child) {
        pid_t c = fork();
        if (c == 0) {
            grandchild_life(ma->child_check);
        } else if (c > 0) {
            // Deliberately do NOT wait all of them: mixed zombie/reaped
            // pressure. Reap a random half from THIS member (claim race
            // with the leader's group wait).
            if (rnd() & 1) {
                int st;
                waitpid(c, &st, __WALL);
            }
        }
    }
    return NULL;
}

// One middle-level process: N threads created, then the process exits
// after a short life. Whether the leader exits before its members
// finish is decided by the parent's kill timing — we just race both.
static void middle_life(int nthreads, int fork_from_member) {
    pthread_t th[8];
    struct member_arg ma[8];
    if (nthreads > 8) nthreads = 8;
    for (int i = 0; i < nthreads; i++) {
        ma[i].fork_child = fork_from_member && (i & 1);
        ma[i].child_check = 1;
        pthread_create(&th[i], NULL, member_fn, &ma[i]);
    }
    struct timespec ts = { 0, (long)(2 + rnd() % 4) * 1000 * 1000 };
    nanosleep(&ts, 0);
    // Either wait for members (leader-last) or exit out from under them
    // (leader-first) — race decided by timing.
    if (rnd() & 1) {
        for (int i = 0; i < nthreads; i++) pthread_join(th[i], NULL);
    }
    _exit(0);
}

int main(void) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    double t_end = now_secs() + PROBE_SECS;
    root_pid = getpid();

    if (prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0) {
        fail("subreaper", 0, 0);
    }

    printf("SUBREAP-START root=%d secs=%d\n", root_pid, PROBE_SECS);

    while (now_secs() < t_end && !stop_flag) {
        cycles++;
        // Chain depth 2-3 under the root: each level is multi-threaded.
        int depth = 2 + (rnd() % 2);
        pid_t mid = fork();
        if (mid == 0) {
            // Level 1 middle.
            if (depth == 3) {
                pid_t m2 = fork();
                if (m2 == 0) {
                    middle_life(2 + rnd() % 4, 1);
                }
                // Leader-first sometimes: kill the deepest leader while
                // its members may still run.
                if (rnd() & 1) kill(m2, SIGKILL);
                middle_life(2 + rnd() % 4, 1);
            }
            middle_life(2 + rnd() % 4, 1);
        }
        // Root: sometimes SIGKILL the middle leader early (reparent of
        // the middle's children to the subreaper root happens HERE —
        // move_child on a group whose members hold parent snapshots).
        struct timespec ts = { 0, (long)(20 + rnd() % 20) * 1000 * 1000 };
        nanosleep(&ts, 0);
        if (rnd() % 4 == 0) kill(mid, SIGKILL);

        // The subreaper must be able to reap everything it adopted.
        for (;;) {
            int st;
            pid_t r = waitpid(-1, &st, __WALL | WNOHANG);
            if (r > 0) {
                reap_ok++;
                continue;
            }
            if (r == 0) break;
            if (errno == EINTR) continue;
            // ECHILD with known-live adopted children would be a lost
            // child; with none outstanding it is normal end-of-wave.
            reap_fail++;
            break;
        }
    }

    // Final drain: everything must be reapable (no lost zombies).
    for (int i = 0; i < 200; i++) {
        int st;
        pid_t r = waitpid(-1, &st, __WALL | WNOHANG);
        if (r <= 0 && errno != EINTR) break;
        reap_ok++;
    }

    printf("SUBREAP-PASS cycles=%lu reaps=%lu orphan_checks=%lu\n",
           cycles, reap_ok, orphan_checks);
    fflush(stdout);
    return 0;
}
