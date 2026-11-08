/* shmprobe — cross-process SysV shm sharing probe (the MIT-SHM shape).
 *
 * The X server reads the client's shared-memory pixmap via shmat of the
 * client's segment id. If the sharing semantics broke (a fresh zero
 * segment instead of the shared pages), the server reads zeros and the
 * screen stays black with everything healthy — the exact black-screen
 * face. Model it: parent shmget+writes, forked child shmat+reads and
 * writes back, parent verifies both directions.
 *
 * Exit code = number of failures.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/ipc.h>
#include <sys/shm.h>
#include <sys/wait.h>
#include <unistd.h>

static int fails;

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);

    for (int round = 0; round < 50; round++) {
        int id = shmget(IPC_PRIVATE, 0x10000, IPC_CREAT | 0666);
        if (id < 0) {
            printf("SHMP round %d: shmget failed: %s\n", round, strerror(errno));
            fails++;
            continue;
        }
        char *p = shmat(id, NULL, 0);
        if (p == (void *)-1) {
            printf("SHMP round %d: parent shmat failed: %s\n", round, strerror(errno));
            fails++;
            shmctl(id, IPC_RMID, NULL);
            continue;
        }
        memset(p, 0, 0x10000);
        for (int i = 0; i < 256; i++) p[i] = (char)(0xA0 + (round % 20));

        pid_t c = fork();
        if (c == 0) {
            char *q = shmat(id, NULL, 0);
            if (q == (void *)-1) _exit(1);
            /* verify parent's pattern */
            int bad = 0;
            for (int i = 0; i < 256; i++) if (q[i] != (char)(0xA0 + (round % 20))) bad++;
            /* write our own at 0x8000 */
            for (int i = 0; i < 256; i++) q[0x8000 + i] = (char)(0x50 + (round % 20));
            shmdt(q);
            _exit(bad ? 2 : 0);
        }
        int st = 0;
        waitpid(c, &st, 0);
        if (!(WIFEXITED(st) && WEXITSTATUS(st) == 0)) {
            printf("SHMP round %d: child verify FAILED st=%#x\n", round, st);
            fails++;
        }
        /* verify child's write from parent side */
        int bad2 = 0;
        for (int i = 0; i < 256; i++) if (p[0x8000 + i] != (char)(0x50 + (round % 20))) bad2++;
        if (bad2) {
            printf("SHMP round %d: parent sees child pattern CORRUPT/zero (%d bad)\n", round, bad2);
            fails++;
        }
        shmdt(p);
        shmctl(id, IPC_RMID, NULL);
    }
    printf("SHMP-DONE fails=%d\n", fails);
    return fails;
}
