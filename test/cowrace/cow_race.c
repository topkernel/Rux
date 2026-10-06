// cow_race.c — reproduce the fake-OOM COW race (layer 2).
//
// Mechanism under test (kernel bug):
//   fork() marks writable pages RO+COW in BOTH parent and child.
//   Two threads of the SAME mm writing the same freshly-COWed page fault
//   concurrently on different harts. handle_mm_fault() checks is_cow_page()
//   lock-free -> CowPending for both. handle_cow_fault() re-walks under
//   PTE_MODIFY_LOCK: the winner breaks COW, the loser finds the COW bit
//   cleared and returns None -> exception.rs maps None to OutOfMemory ->
//   SIGKILL ("pagefault: Out of memory") with plenty of free memory.
//
// Shape: rounds of (fork + immediate child exit -> page re-COWed for next
// round via the NEXT fork; barrier; N threads hammer the same bytes).
// PASS if we never die and print COWRACE-PASS.
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/wait.h>

#define NTHREADS 4
#define ROUNDS 400

static char *target;      /* own page: written by all threads */
static volatile int go;
static volatile int stop;

static void *worker(void *arg)
{
    long id = (long)arg;
    (void)id;
    for (;;) {
        while (!go && !stop)
            ;
        if (stop && !go)
            break;
        for (int i = 0; i < 64; i++)
            target[16]++;          /* same byte from every thread */
        break;                     /* one burst per round; main resets go */
    }
    return NULL;
}

int main(void)
{
    pthread_t th[NTHREADS];
    if (posix_memalign((void **)&target, 4096, 4096) != 0) { printf("COWRACE-FAIL alloc\n"); return 1; }
    memset((void *)target, 1, 4096);
    /* warm the mapping so fork() actually COWs it */
    target[16] = 7;

    setvbuf(stdout, NULL, _IONBF, 0);
    printf("COWRACE-START threads=%d rounds=%d\n", NTHREADS, ROUNDS);

    for (int r = 0; r < ROUNDS; r++) {
        pid_t pid = fork();
        if (pid < 0) {
            printf("COWRACE-FAIL fork at round %d\n", r);
            return 1;
        }
        if (pid == 0)
            _exit(0);              /* child: exit, dropping its COW share */
        waitpid(pid, NULL, 0);
        /* parent PTEs are now RO+COW (fork walk downgraded them) */

        for (long i = 0; i < NTHREADS; i++)
            pthread_create(&th[i], NULL, worker, (void *)i);
        go = 1;                    /* release all at once */
        for (int i = 0; i < NTHREADS; i++)
            pthread_join(th[i], NULL);

        if ((r % 50) == 0)
            printf("COWRACE-ROUND %d\n", r);
    }
    stop = 1;
    printf("COWRACE-PASS val=%d\n", target[16]);
    return 0;
}
