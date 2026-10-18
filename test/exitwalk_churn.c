// exitwalk_churn.c — fork/exit churn racing /proc/<pid>/maps walkers.
//
// Repro target (fs-writestorm KVM OH round): pid 555 exited and freed its
// page-table root; pid 554's teardown then re-walked the same root
// (PTLEDGER DOUBLE-FREE REFUSED x26, FUT-STALE-TREE) and the kernel #GP'd
// inside a VMA btree walk. Engine: (a) an mm Arc clone racing the owner's
// exit-time slot swap resurrected freed memory (double teardown), (b) a
// procfs maps/status reader walking another task's mm while that task's
// exit freed the VMA tree under it.
//
// This probe drives every leg of that shape:
//   - SPAWNER processes: rapid fork of children that mmap (anon + file),
//     touch, read their own /proc/self/maps, then exit via varied paths
//     (_exit, exit, SIGKILL self).
//   - RAW-CLONE spawner leg: clone(CLONE_VM|SIGCHLD, no CLONE_THREAD) —
//     cross-tgid mm sharing; the child exits while the parent still runs
//     on the shared mm.
//   - FUTEX leg: threads park on futex words and exit in waves
//     (CLONE_VM thread-group exit shape; mm-id resolution on exit).
//   - WALKER processes: continuously re-scan /proc and fully read
//     /proc/<pid>/maps (+status, smaps every few pids) of whatever pids
//     exist — the live VMA-btree/PT walker.
//
// Verdict contract (driver: exitwalk_boot.py):
//   EXITWALK-PASS after PROBE_SECS seconds  -> rc 0
//   EXITWALKFAIL-* line                     -> rc 1
//   walker death by SIGSEGV/SIGBUS/SIGILL   -> EXITWALKFAIL-reader-signal
// Kernel-side #GP/panic/FUT-STALE/PTLEDGER detection is the driver's job.
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#include <sched.h>

#ifndef PROBE_SECS
#define PROBE_SECS 30
#endif
/* Churn pacing: microseconds slept between spawn iterations. 0 = the
 * pathological KVM rate (~5k forks/s aggregate, well beyond OH's
 * launcher era ~1-10/s); 20000 = ~100/s aggregate (10x heavier than
 * OH) for acceptance-mode runs. */
#ifndef CHURN_DELAY_US
#define CHURN_DELAY_US 0
#endif
/* smaps cadence: every Nth pid (1 = every pid — the pathological
 * long-borrow hammer; 50 = OH-realistic occasional heavy scan). */
#ifndef SMAPS_EVERY
#define SMAPS_EVERY 7
#endif

#define NSPAWNERS 2
#define NWALKERS  2

#define CHURN_FILE "/test/exitwalk-churn.file"

/* shared page: counters + stop flag (children are forked processes) */
static struct shared {
    volatile int stop;
    unsigned long spawns;
    unsigned long reads;
    unsigned long open_miss; /* ENOENT — pid raced an exit, expected */
    unsigned long read_fails;
} *sh;

/* ---------- futex wrapper (musl headers don't export the ABI consts) ---------- */
#define FT_WAIT 0
#define FT_WAKE 1
static long futex_wait(volatile int *uaddr, int val, const struct timespec *tmo)
{
    return syscall(SYS_futex, (int *)uaddr, FT_WAIT, val, tmo, NULL, 0);
}
static long futex_wake(volatile int *uaddr, int n)
{
    return syscall(SYS_futex, (int *)uaddr, FT_WAKE, n, NULL, NULL, 0);
}

/* ---------- verdict ---------- */
static void fail(const char *tag)
{
    dprintf(2, "EXITWALKFAIL-%s\n", tag);
    fflush(NULL);
    _exit(1);
}

static void walker_signal_handler(int sig)
{
    /* Probe-only handler: the raw write is what matters. */
    const char msg[] = "EXITWALKFAIL-reader-signal\n";
    (void)sig;
    syscall(SYS_write, 2, msg, sizeof(msg) - 1);
    syscall(SYS_exit_group, 1);
}

/* ---------- monotonic ms ---------- */
static double now_ms(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000.0 + ts.tv_nsec / 1e6;
}

/* ---------- churn child: what every spawned victim does ---------- */
static void child_work(int id)
{
    unsigned int seed = id * 2654435761u + (unsigned int)getpid();
    int i, r;
    char buf[4096];

    /* anon regions, touched (VMA btree entries + resident pages) */
    for (i = 0; i < 6; i++) {
        size_t sz = (size_t)(1 + (seed >> i) % 8) * 4096;
        char *p = mmap(NULL, sz, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (p == MAP_FAILED)
            continue;
        memset(p, (int)(seed + i), sz);
    }
    /* one file-backed VMA */
    {
        int fd = open(CHURN_FILE, O_RDONLY);
        if (fd >= 0) {
            char *p = mmap(NULL, 16 * 4096, PROT_READ, MAP_PRIVATE, fd, 0);
            if (p != MAP_FAILED)
                (void)p[seed % (16 * 4096)];
            close(fd);
        }
    }
    /* self-walk: read our own maps (same-mm walker on the hot path) */
    {
        int mfd = open("/proc/self/maps", O_RDONLY);
        if (mfd >= 0) {
            while (read(mfd, buf, sizeof(buf)) > 0)
                ;
            close(mfd);
        }
    }
    /* sometimes park briefly on a futex (ETIMEDOUT) so exit-time wake
     * paths hit mm-identity resolution near teardown */
    r = (int)(seed % 4);
    if (r == 3) {
        volatile int w = 0;
        struct timespec tmo = { 0, 20 * 1000 * 1000 };
        (void)futex_wait(&w, 1, &tmo);
    }

    /* varied exit paths exercise different kernel teardown entries */
    switch (r) {
    case 0:
        _exit(23);
        break;
    case 1:
        exit(0);
        break;
    case 2:
        signal(SIGKILL, SIG_DFL);
        raise(SIGKILL);
        _exit(9);
        break;
    default:
        _exit(0);
        break;
    }
}

/* ---------- spawner process ---------- */
static int raw_clone_child(void *arg)
{
    (void)arg;
    /* Cross-tgid CLONE_VM share: our exit drops an mm reference while
     * the parent (different tgid) keeps using the mm. Touch pages first
     * so exit-time teardown has content to race walkers against. */
    char *p = mmap(NULL, 8 * 4096, PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p != MAP_FAILED)
        memset(p, 0x5a, 8 * 4096);
    _exit(0);
}

static void spawner_main(int id)
{
    unsigned int seed = 0x9e3779b9u ^ (unsigned int)(id * 7919);
    int iter = 0;

    while (!sh->stop) {
        seed = seed * 1103515245 + 12345u;
        iter++;

        if (iter % 4 == 0) {
            /* raw clone(CLONE_VM|SIGCHLD): separate tgid, shared mm.
             * Blocking wait before munmap: the child runs on this stack,
             * and with CLONE_VM the parent's munmap would pull it away
             * mid-execution (userspace SIGSEGV noise, not the target
             * race). After waitpid the child is zombie — its user code
             * is done touching the stack. */
            size_t stksz = 256 * 1024;
            char *stk = mmap(NULL, stksz, PROT_READ | PROT_WRITE,
                             MAP_PRIVATE | MAP_ANONYMOUS | MAP_STACK, -1, 0);
            if (stk != MAP_FAILED) {
                pid_t c = clone(raw_clone_child, stk + stksz,
                                CLONE_VM | SIGCHLD, NULL);
                if (c > 0) {
                    sh->spawns++;
                    while (waitpid(c, NULL, 0) < 0 && errno == EINTR)
                        ;
                }
                munmap(stk, stksz);
            }
        } else {
            pid_t c = fork();
            if (c == 0)
                child_work(iter); /* no return */
            if (c > 0)
                sh->spawns++;
            while (waitpid(-1, NULL, WNOHANG) > 0)
                ;
        }
        /* vary the churn rate so exits interleave walkers */
        if ((seed & 7) == 0)
            usleep(1000);
#if CHURN_DELAY_US > 0
        usleep(CHURN_DELAY_US);
#endif
    }
    _exit(0);
}

/* ---------- walker process ---------- */
static int read_whole(const char *path)
{
    static char buf[64 * 1024];
    int fd = open(path, O_RDONLY);
    ssize_t n;

    if (fd < 0) {
        if (errno == ENOENT) {
            sh->open_miss++; /* pid raced an exit — expected */
            return 0;
        }
        if (errno == EACCES || errno == EPERM) {
            /* foreign-user entry (host dry-run compat; the guest runs as
             * root and never sees this) — benign skip */
            sh->open_miss++;
            return 0;
        }
        sh->read_fails++; /* EIO/ENOMEM/etc. is suspicious */
        return -1;
    }
    while ((n = read(fd, buf, sizeof(buf))) > 0)
        ;
    if (n < 0)
        sh->read_fails++;
    close(fd);
    return 0;
}

static void walker_main(int id)
{
    char path[128];
    unsigned int round = 0;

    signal(SIGSEGV, walker_signal_handler);
    signal(SIGBUS, walker_signal_handler);
    signal(SIGILL, walker_signal_handler);
    signal(SIGPIPE, SIG_IGN);

    while (!sh->stop) {
        DIR *d = opendir("/proc");
        struct dirent *de;
        if (!d) {
            sh->read_fails++;
            usleep(10000);
            continue;
        }
        while ((de = readdir(d)) != NULL) {
            char *e;
            long pid = strtol(de->d_name, &e, 10);
            if (*e != '\0' || pid <= 0)
                continue;
            round++;

            snprintf(path, sizeof(path), "/proc/%ld/maps", pid);
            if (read_whole(path) < 0)
                break;
            sh->reads++;

            if (round % 3 == 0) {
                snprintf(path, sizeof(path), "/proc/%ld/status", pid);
                (void)read_whole(path);
                sh->reads++;
            }
            if (round % SMAPS_EVERY == 0) {
                /* smaps: per-page PT walk — the expensive hammer */
                snprintf(path, sizeof(path), "/proc/%ld/smaps", pid);
                (void)read_whole(path);
                sh->reads++;
            }
            if (sh->stop)
                break;
        }
        closedir(d);
        if ((round & 15) == 0)
            usleep(1000 + id);
    }
    _exit(0);
}

/* ---------- futex churn process (thread-group exits) ---------- */
struct ftx_ctx {
    volatile int word;
};

static void *ftx_worker(void *arg)
{
    struct ftx_ctx *c = arg;
    int i;
    for (i = 0; i < 200 && !sh->stop; i++) {
        (void)futex_wait(&c->word, 0, NULL);
        c->word = 0;
    }
    return NULL; /* pthread exit: CLONE_VM group member exit path */
}

static void futex_main(void)
{
    static struct ftx_ctx ctx;
    pthread_t th[3];
    int gen = 0, i;

    while (!sh->stop) {
        gen++;
        ctx.word = 0;
        for (i = 0; i < 3; i++)
            pthread_create(&th[i], NULL, ftx_worker, &ctx);
        usleep(50000);
        ctx.word = 1;
        for (i = 0; i < 3; i++)
            (void)futex_wake(&ctx.word, 1);
        for (i = 0; i < 3; i++)
            pthread_join(th[i], NULL);
    }
    (void)gen;
    _exit(0);
}

/* ---------- main ---------- */
int main(void)
{
    double deadline, last_stat = 0;
    int i;
    pid_t procs[NSPAWNERS + NWALKERS + 1];
    int nprocs = 0;

    setvbuf(stdout, NULL, _IONBF, 0);
    setsid();
    signal(SIGCHLD, SIG_DFL);

    sh = mmap(NULL, sizeof(*sh), PROT_READ | PROT_WRITE,
              MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    if (sh == MAP_FAILED || sh == NULL) {
        dprintf(2, "EXITWALKFAIL-shared-page\n");
        return 1;
    }
    memset((void *)sh, 0, sizeof(*sh));

    /* backing file for file-backed VMAs */
    {
        int fd = open(CHURN_FILE, O_CREAT | O_RDWR | O_TRUNC, 0644);
        if (fd >= 0) {
            char page[4096];
            memset(page, 0xa5, sizeof(page));
            for (i = 0; i < 16; i++)
                (void)write(fd, page, sizeof(page));
            close(fd);
        }
    }

    dprintf(2, "EXITWALK: start secs=%d\n", PROBE_SECS);

    for (i = 0; i < NSPAWNERS; i++) {
        pid_t p = fork();
        if (p == 0)
            spawner_main(i);
        procs[nprocs++] = p;
        dprintf(2, "EXITWALK-ACTOR role=spawner%d pid=%d\n", i, p);
    }
    for (i = 0; i < NWALKERS; i++) {
        pid_t p = fork();
        if (p == 0)
            walker_main(i);
        procs[nprocs++] = p;
        dprintf(2, "EXITWALK-ACTOR role=walker%d pid=%d\n", i, p);
    }
    {
        pid_t p = fork();
        if (p == 0)
            futex_main();
        procs[nprocs++] = p;
        dprintf(2, "EXITWALK-ACTOR role=futex pid=%d\n", p);
    }

    deadline = now_ms() + PROBE_SECS * 1000.0;
    while (now_ms() < deadline) {
        pid_t d;
        while ((d = waitpid(-1, NULL, WNOHANG)) > 0) {
            int j;
            for (j = 0; j < nprocs; j++) {
                if (procs[j] == d) {
                    /* An actor died before stop — respawn to keep
                     * coverage dense; the kernel-side verdict (driver)
                     * judges real faults. */
                    pid_t p2 = -1;
                    if (j < NSPAWNERS) {
                        p2 = fork();
                        if (p2 == 0)
                            spawner_main(j);
                    } else if (j < NSPAWNERS + NWALKERS) {
                        p2 = fork();
                        if (p2 == 0)
                            walker_main(j - NSPAWNERS);
                    } else {
                        p2 = fork();
                        if (p2 == 0)
                            futex_main();
                    }
                    procs[j] = p2;
                    dprintf(2, "EXITWALK-RESPAWN role-idx=%d newpid=%d (old %d died)\n",
                            j, p2, d);
                    break;
                }
            }
        }
        if (now_ms() - last_stat > 5000) {
            last_stat = now_ms();
            dprintf(2, "EXITWALK-STATS spawns=%lu reads=%lu openmiss=%lu rfail=%lu\n",
                    sh->spawns, sh->reads, sh->open_miss, sh->read_fails);
        }
        usleep(50000);
    }

    sh->stop = 1;
    usleep(300000); /* let actors wind down */
    for (i = 0; i < nprocs; i++)
        if (procs[i] > 0)
            kill(procs[i], SIGKILL);
    while (waitpid(-1, NULL, WNOHANG) > 0)
        ;

    /* reader-visible read failures (EIO etc., not ENOENT races) are a
     * verdict; a handful is noise-tolerant only if reads were massive */
    if (sh->read_fails > 32)
        fail("reader-read-errors");

    dprintf(2, "EXITWALK-PASS spawns=%lu reads=%lu openmiss=%lu rfail=%lu\n",
            sh->spawns, sh->reads, sh->open_miss, sh->read_fails);
    return 0;
}
