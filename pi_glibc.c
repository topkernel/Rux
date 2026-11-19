// pi_glibc.c — glibc PTHREAD_PRIO_INHERIT + ROBUST UAPI chain probe
// (R6, rt-r6 lane). Exercises the whole chain the plan's starting
// table called "PI-mutex UAPI chain fails":
//
//  Part A (boost): a FIFO-80 thread and a SCHED_OTHER thread contend a
//  PTHREAD_PRIO_INHERIT mutex; while the CFS holder is inside its
//  critical section and the FIFO waiter is queued, the holder must run
//  as SCHED_FIFO 80 (glibc FUTEX_LOCK_PI underneath). Verified from
//  the main thread via sched_getscheduler.
//
//  Part B (robust death, in-process): a FIFO thread takes the robust
//  PI mutex and pthread_exit()s WITHOUT unlocking. The next locker
//  must get EOWNERDEAD, run pthread_mutex_consistent(), and continue.
//
//  Part C (robust death, kill): a fork()ed child holds the PROCESS-
//  SHARED robust PI mutex (in mmap'd shared memory) and pauses; the
//  parent SIGKILLs it; the parent's next lock must return EOWNERDEAD
//  (robust list processed at exit) and recovery must succeed.
//
// Usage: pi_glibc
// Verdict: RES|pi_glibc|PASS|boost=<0/1> died_inproc=<0/1> died_kill=<0/1>

#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <pthread.h>
#include <sched.h>
#include <signal.h>
#include <time.h>
#include <sys/wait.h>
#include <sys/mman.h>
#include <sys/syscall.h>

static void msleep(long ms)
{
    struct timespec ts = { .tv_sec = ms / 1000,
                           .tv_nsec = (ms % 1000) * 1000000L };
    nanosleep(&ts, NULL);
}

static void set_fifo(int prio)
{
    struct sched_param sp = { .sched_priority = prio };
    if (sched_setscheduler(0, SCHED_FIFO, &sp) != 0)
        perror("sched_setscheduler(FIFO)");
}

// Shared segment: the process-shared robust PI mutex + the phase gate
// the forked child signals through.
static struct shared {
    pthread_mutex_t m;
    volatile int phase;
} *shm;

static volatile pid_t holder_tid;
static volatile int a_phase;

// ---------- Part A + B workers ----------

static void *cfs_holder(void *ap)
{
    (void)ap;
    holder_tid = syscall(SYS_gettid);
    while (a_phase < 1)
        msleep(1);
    pthread_mutex_lock(&shm->m);
    a_phase = 2;
    while (a_phase < 3)
        msleep(1);
    pthread_mutex_unlock(&shm->m);
    a_phase = 4;
    return NULL;
}

static void *fifo_waiter(void *ap)
{
    (void)ap;
    set_fifo(80);
    while (a_phase < 2)
        msleep(1);
    pthread_mutex_lock(&shm->m); // blocks; boosts the CFS holder
    pthread_mutex_unlock(&shm->m);
    return NULL;
}

static void *dying_holder(void *ap)
{
    (void)ap;
    set_fifo(60);
    pthread_mutex_lock(&shm->m);
    a_phase = 10; // tell main we hold it
    pthread_exit(NULL); // dies WITHOUT unlocking (robust path)
    return NULL;
}

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    int boost_ok = 0, died_inproc = 0, died_kill = 0;

    printf("G-PART0 mmap\n");
    shm = mmap(NULL, sizeof(*shm), PROT_READ | PROT_WRITE,
               MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    if (shm == MAP_FAILED) {
        perror("mmap");
        return 2;
    }
    pthread_mutexattr_t a;
    pthread_mutexattr_init(&a);
    pthread_mutexattr_setprotocol(&a, PTHREAD_PRIO_INHERIT);
    pthread_mutexattr_setrobust(&a, PTHREAD_MUTEX_ROBUST);
    pthread_mutexattr_setpshared(&a, PTHREAD_PROCESS_SHARED);
    printf("G-PART0 mutex-init\n");
    pthread_mutex_init(&shm->m, &a);
    shm->phase = 0;
    printf("G-PART0 done\n");

    // ---- Part A: PI boost through glibc ----
    {
        a_phase = 0;
        pthread_t h, w;
        pthread_create(&h, NULL, cfs_holder, NULL);
        while (holder_tid == 0)
            msleep(1);
        pthread_create(&w, NULL, fifo_waiter, NULL);
        a_phase = 1;
        printf("G-PARTA holder-locked\n");
        while (a_phase < 2)
            msleep(1);
        msleep(300); // let the FIFO waiter block and the boost arm

        struct sched_param sp;
        int pol = sched_getscheduler(holder_tid);
        memset(&sp, 0, sizeof(sp));
        sched_getparam(holder_tid, &sp);
        boost_ok = (pol == SCHED_FIFO && sp.sched_priority == 80);
        if (!boost_ok)
            fprintf(stderr, "GLIBC-FAIL boost: pol=%d prio=%d want FIFO/80\n",
                    pol, sp.sched_priority);

        a_phase = 3;
        pthread_join(h, NULL);
        pthread_join(w, NULL);
        msleep(100);
    }

    // ---- Part B: in-process robust death ----
    {
        printf("G-PARTB start\n");
        a_phase = 0;
        pthread_t d;
        pthread_create(&d, NULL, dying_holder, NULL);
        while (a_phase < 10)
            msleep(1);
        msleep(100); // let the exit robust walk finish

        int rc = pthread_mutex_lock(&shm->m);
        if (rc == EOWNERDEAD) {
            if (pthread_mutex_consistent(&shm->m) == 0) {
                pthread_mutex_unlock(&shm->m);
                died_inproc = 1;
            } else {
                fprintf(stderr, "GLIBC-FAIL consistent(inproc): %s\n",
                        strerror(errno));
            }
        } else {
            fprintf(stderr, "GLIBC-FAIL lock(inproc) rc=%d %s want EOWNERDEAD(%d)\n",
                    rc, strerror(rc), EOWNERDEAD);
        }
        pthread_join(d, NULL);
    }

    // ---- Part C: cross-process robust death (SIGKILL) ----
    {
        printf("G-PARTC start\n");
        fflush(NULL);
        shm->phase = 0;
        pid_t p = fork();
        if (p == 0) {
            // child: take the shared mutex, then pause until killed.
            set_fifo(50);
            int rc = pthread_mutex_lock(&shm->m);
            if (rc == EOWNERDEAD) {
                if (pthread_mutex_consistent(&shm->m) != 0)
                    _exit(43);
            } else if (rc != 0) {
                _exit(42);
            }
            shm->phase = 20;
            pause();
            _exit(0);
        }
        int spins = 0;
        while (shm->phase < 20 && spins++ < 10000)
            msleep(50);
        msleep(100);
        kill(p, SIGKILL);
        int st;
        waitpid(p, &st, 0);
        if (!WIFSIGNALED(st))
            fprintf(stderr, "GLIBC-FAIL child not killed: st=%d\n", st);
        msleep(100); // robust processing happened in the child's exit

        int rc = pthread_mutex_lock(&shm->m);
        if (rc == EOWNERDEAD) {
            if (pthread_mutex_consistent(&shm->m) == 0) {
                pthread_mutex_unlock(&shm->m);
                died_kill = 1;
            } else {
                fprintf(stderr, "GLIBC-FAIL consistent(kill): %s\n",
                        strerror(errno));
            }
        } else {
            fprintf(stderr, "GLIBC-FAIL lock(kill) rc=%d %s want EOWNERDEAD(%d)\n",
                    rc, strerror(rc), EOWNERDEAD);
        }
    }

    int pass = boost_ok && died_inproc && died_kill;
    printf("RES|pi_glibc|%s|boost=%d died_inproc=%d died_kill=%d\n",
           pass ? "PASS" : "FAIL", boost_ok, died_inproc, died_kill);
    return pass ? 0 : 1;
}
