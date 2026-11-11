// waitloss_probe.c — wait4/SIGCHLD wakeup-loss detector (99th, TASK §87.8)
//
// Shape hunted: a parent blocked in wait4/waitpid never wakes when its
// child exits. The child's exit ran; the parent stays parked. Intermittent
// ~50% on multi-wait scripts (hp87g conviction: sleep-anchored rigs
// complete, wait-dependent rigs die).
//
// Phases (each prints progress around the fork AND the wait, so a stall
// names its exact stage):
//   A  200 x fork + child _exit(0) IMMEDIATELY + waitpid(specific pid)
//      — the tightest exit-vs-park race (the `cmd; next` shape).
//   B  100 x fork + child busy-delays then exits + waitpid — varies the
//      child-exit time across the parent's scan/park window.
//   C   50 x fork 4 children + waitpid(-1) x4 — the multi-wait shape.
//
// A watchdog CHILD (forked, shares an mmap page) re-prints the parent's
// frozen (phase, iter, stage) every 2 s: a guest that keeps printing WD
// lines while the parent never advances = wake LOST (kernel alive,
// parent parked); a fully silent guest = deeper wedge (panic/hang).
//
// WAITLOSS-DONE at the end = the detector passed.

#include <sys/mman.h>
#include <sys/wait.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

struct shared {
    volatile int phase;   // 0=A 1=B 2=C 3=done 4=fail
    volatile int iter;    // current iteration in phase
    volatile int stage;   // 0=before-fork 1=child-running 2=in-wait
    volatile int pid_hint;// child pid of current iteration
};

static void wd_line(struct shared *s)
{
    char buf[96];
    int n = snprintf(buf, sizeof buf, "WD phase=%d iter=%d stage=%d child=%d\n",
                     s->phase, s->iter, s->stage, s->pid_hint);
    write(1, buf, n);
}

// Load generators (99th: the idle-guest probe passed 10/10 at SMP2; the
// §87.8 stalls live under contention). Each loader child loops
// creat/write/fsync/unlink churn on /tmp plus fork/exit round trips —
// disk completions (IRQ-context wait-queue wakes) + exit-notify churn +
// scheduler pressure, the mix the OH boot and the fs-integrity rigs ran.
static void run_loaders(int n)
{
    for (int k = 0; k < n; k++) {
        pid_t p = fork();
        if (p == 0) {
            char path[64];
            snprintf(path, sizeof path, "/tmp/load%d", k);
            for (unsigned i = 0;; i++) {
                int fd = open(path, 01 | 0100 | 01000, 0644); /* wr|cr|tr */
                if (fd >= 0) {
                    char b[128];
                    int n2 = snprintf(b, sizeof b, "load %u %u\n", k, i);
                    write(fd, b, n2);
                    fsync(fd);
                    close(fd);
                    unlink(path);
                }
                if ((i & 0xF) == 0xF) {
                    /* periodic fork+exit churn of our own */
                    pid_t q = fork();
                    if (q == 0)
                        _exit(0);
                    if (q > 0) {
                        int st;
                        while (waitpid(q, &st, 0) != q)
                            ;
                    }
                }
            }
        }
    }
}

int main(int argc, char **argv)
{
    struct shared *s = mmap(NULL, 4096, PROT_READ | PROT_WRITE,
                            MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    if (s == MAP_FAILED) {
        static const char m[] = "WD mmap-failed\n";
        write(1, m, sizeof m - 1);
        return 4;
    }
    s->phase = 0; s->iter = 0; s->stage = 0; s->pid_hint = 0;

    if (argc > 1) {
        int n = atoi(argv[1]);
        if (n > 0) {
            printf("loaders=%d\n", n);
            run_loaders(n);
            sleep(1); /* let the churn spin up */
        }
    }

    // Watchdog child: prints the parent's progress every 2 s. Exits when
    // the parent sets phase=3 (done) or phase=4 (child-side failure).
    pid_t wd = fork();
    if (wd == 0) {
        for (;;) {
            sleep(2);
            wd_line(s);
            if (s->phase >= 3)
                _exit(0);
        }
    }

    setvbuf(stdout, NULL, _IONBF, 0);

    // ---- Phase A: tight fork + immediate exit + specific waitpid ----
    for (int i = 0; i < 200; i++) {
        s->phase = 0; s->iter = i; s->stage = 0; s->pid_hint = 0;
        printf("A %d fork\n", i);
        pid_t p = fork();
        if (p == 0)
            _exit(0);
        if (p < 0) {
            printf("A %d fork-failed\n", i);
            s->phase = 4;
            goto out;
        }
        s->stage = 2; s->pid_hint = p;
        printf("A %d wait %d\n", i, (int)p);
        int st = 0;
        pid_t r = waitpid(p, &st, 0);
        if (r != p) {
            printf("A %d wait-rc=%d unexpected\n", i, (int)r);
            s->phase = 4;
            goto out;
        }
    }

    // ---- Phase B: delayed child exit (exit races the parent's park) ----
    for (int i = 0; i < 100; i++) {
        s->phase = 1; s->iter = i; s->stage = 0; s->pid_hint = 0;
        pid_t p = fork();
        if (p == 0) {
            // deterministic-ish user-space delay: no timer dependency
            volatile unsigned d = 1000u * (unsigned)(i + 1);
            while (d--)
                __asm__ volatile("" ::: "memory");
            _exit(0);
        }
        if (p < 0) {
            printf("B %d fork-failed\n", i);
            s->phase = 4;
            goto out;
        }
        s->stage = 2; s->pid_hint = p;
        int st = 0;
        pid_t r = waitpid(p, &st, 0);
        if (r != p) {
            printf("B %d wait-rc=%d unexpected\n", i, (int)r);
            s->phase = 4;
            goto out;
        }
        if (i % 10 == 0)
            printf("B %d ok\n", i);
    }

    // ---- Phase C: 4 concurrent children, waitpid(-1) ----
    for (int i = 0; i < 50; i++) {
        s->phase = 2; s->iter = i; s->stage = 0; s->pid_hint = 0;
        pid_t ps[4];
        for (int k = 0; k < 4; k++) {
            ps[k] = fork();
            if (ps[k] == 0)
                _exit(k);
        }
        s->stage = 2;
        for (int k = 0; k < 4; k++) {
            int st = 0;
            pid_t r = waitpid(-1, &st, 0);
            if (r < 0) {
                printf("C %d wait-rc=%d\n", i, (int)r);
                s->phase = 4;
                goto out;
            }
        }
        if (i % 10 == 0)
            printf("C %d ok\n", i);
    }

    s->phase = 3;
    printf("WAITLOSS-DONE\n");
out:
    {
        int st = 0;
        waitpid(wd, &st, 0);
    }
    return s->phase == 3 ? 0 : 5;
}
