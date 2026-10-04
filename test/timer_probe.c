/* timer_probe.c — minimal timer-wakeup-loss reproducer (no sockets).
 *
 * Hypothesis under test: the unix_wedge PROC-STALL(epw, state=S) wedge is a
 * pure TIMER-side loss (one-shot wake timer never delivered), independent
 * of AF_UNIX. This probe reproduces the exact load shape of the wedged
 * child — high-frequency SHORT nanosleeps (20-60ms) — with nothing else:
 * no sockets, no poll, no signals.
 *
 *   N sleepers : loop { nanosleep(20-60ms); stamp shm heartbeat }
 *   supervisor : waits, scans heartbeats every 5s; a heartbeat frozen
 *                > 3s while /proc state == 'S' is TIMER-WAKEUP-LOST.
 *                state == 'R' would be a scheduler/spin bug.
 *
 * Usage: timer_probe [dur_sec=600] [nsleepers=6] [pollers=2]
 * Output contract:
 *   TIMER_PROBE RESULT: FAIL kind=TIMER-STALL pid=<n> state=<c> hb_age=<s>
 *   TIMER_PROBE RESULT: PASS sleeps=<n> max_lag=<s>
 * Exit codes: 0 PASS, 1 FAIL, 2 setup error.
 * Compile: riscv64-linux-gnu-gcc -static -O2 -Wall -o timer_probe timer_probe.c
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <time.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <sys/mman.h>
#include <sys/mount.h>

#define MAX_SLEEPERS 32
#define HB_STALE_S   3.0

static int duration = 600;
static int nsleepers = 6;

struct s_shm {
    volatile double hb;
    volatile unsigned long sleeps;
    volatile long state;      /* 1 run 3 stalled */
};
struct probe_shm {
    volatile double start;
    struct s_shm s[MAX_SLEEPERS + 1];
};
static struct probe_shm *SHM;

static double mono(void)
{
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) == 0)
        return ts.tv_sec + ts.tv_nsec / 1e9;
    return (double)time(NULL);
}

static char proc_state_char(pid_t pid)
{
    char path[64];
    snprintf(path, sizeof path, "/proc/%d/stat", (int)pid);
    FILE *f = fopen(path, "r");
    if (!f) return '?';
    char buf[512];
    if (!fgets(buf, sizeof buf, f)) { fclose(f); return '?'; }
    fclose(f);
    char *rp = strrchr(buf, ')');
    if (!rp || rp[1] != ' ') return '?';
    return rp[2];
}

static int sleeper_main(int cid)
{
    unsigned seed = (unsigned)(cid * 7919 + 13);
    double t0 = mono();
    SHM->s[cid].state = 1;
    while (mono() - t0 < (double)duration) {
        /* the exact ep_writer shape: 20-60ms sleeps at high frequency */
        long ms = 20 + (long)(rand_r(&seed) % 41);
        struct timespec ts = { 0, ms * 1000000L };
        nanosleep(&ts, NULL);
        SHM->s[cid].sleeps++;
        SHM->s[cid].hb = mono();
    }
    SHM->s[cid].state = 2;
    return 0;
}

int main(int argc, char **argv)
{
    setvbuf(stdout, NULL, _IONBF, 0);

    if (argc > 1) duration = atoi(argv[1]);
    if (argc > 2) nsleepers = atoi(argv[2]);
    if (nsleepers > MAX_SLEEPERS) nsleepers = MAX_SLEEPERS;
    if (duration < 10) duration = 10;

    mkdir("/proc", 0755);
    mount("proc", "/proc", "proc", 0, NULL);

    SHM = mmap(NULL, sizeof *SHM, PROT_READ | PROT_WRITE,
               MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    if (SHM == MAP_FAILED) {
        printf("TIMER_PROBE RESULT: FAIL kind=SETUP(mmap)\n");
        return 2;
    }
    memset((void *)SHM, 0, sizeof *SHM);
    SHM->start = mono();

    printf("TIMER_PROBE CONFIG dur=%d sleepers=%d pid=%d\n",
           duration, nsleepers, getpid());

    pid_t cp[MAX_SLEEPERS + 1];
    for (int i = 1; i <= nsleepers; i++) {
        pid_t p = fork();
        if (p == 0) _exit(sleeper_main(i));
        cp[i] = p;
    }

    double max_lag = 0;
    double t0 = mono(), last_line = t0;
    int fail = 0, finished = 0;
    char first_kind[128] = "";

    while (finished < nsleepers && mono() - t0 < (double)duration + 60.0) {
        int st = 0;
        pid_t p = waitpid(-1, &st, WNOHANG);
        if (p < 0) break;
        if (p == 0) {
            struct timespec ts = { 1, 0 };
            nanosleep(&ts, NULL);
            double now = mono();
            double lag_sum = 0;
            for (int i = 1; i <= nsleepers; i++) {
                if (SHM->s[i].hb > 0)
                    lag_sum = now - SHM->s[i].hb;
                if (lag_sum > max_lag) max_lag = lag_sum;
            }
            if (now - last_line > 10.0) {
                unsigned long total = 0;
                int alive = 0;
                for (int i = 1; i <= nsleepers; i++) {
                    total += SHM->s[i].sleeps;
                    if (SHM->s[i].state == 1) alive++;
                }
                printf("STATUS t=%.0f alive=%d/%d sleeps=%lu "
                       "max_lag=%.2fs\n",
                       now - SHM->start, alive, nsleepers, total, max_lag);
                last_line = now;
            }
            /* stall scan: 3s covers several 60ms sleep cycles */
            for (int i = 1; i <= nsleepers && !fail; i++) {
                if (SHM->s[i].state == 1 && SHM->s[i].hb > 0 &&
                    now - SHM->s[i].hb > HB_STALE_S) {
                    char sc = proc_state_char(cp[i]);
                    printf("TIMER-STALL t=%.0f sleeper=%d pid=%d — heartbeat "
                           "frozen %.1fs after %lu sleeps; /proc state=%c "
                           "(S=sleeping: wake never delivered, R=spin)\n",
                           now - SHM->start, i, cp[i], now - SHM->s[i].hb,
                           SHM->s[i].sleeps, sc);
                    snprintf(first_kind, sizeof first_kind,
                             "TIMER-STALL sleeper=%d state=%c", i, sc);
                    fail = 1;
                }
            }
            continue;
        }
        for (int i = 1; i <= nsleepers; i++) {
            if (cp[i] == p) {
                cp[i] = -1;
                finished++;
                break;
            }
        }
    }

    if (fail) {
        int st;
        for (int i = 1; i <= nsleepers; i++)
            if (cp[i] > 0) kill(cp[i], SIGKILL);
        while (waitpid(-1, &st, WNOHANG) > 0) ;
        printf("TIMER_PROBE RESULT: FAIL kind=%s\n", first_kind);
        return 1;
    }
    printf("TIMER_PROBE RESULT: PASS max_lag=%.2fs\n", max_lag);
    return 0;
}
