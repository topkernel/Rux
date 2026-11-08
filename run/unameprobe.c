/* unameprobe — hammer uname() from 8 threads; catch any glitch in the
 * machine/nodename/release fields (the fields LLVM's getProcessTriple
 * parses to decide 32/64-bit).
 *
 * If any read returns a machine field != "x86_64", the process-subtarget
 * flips to i386 for that reader — the exact "64-bit code requested on a
 * subtarget that doesn't support it!" abort llvmpipe hits.
 */
#define _GNU_SOURCE
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <sys/utsname.h>
#include <stdatomic.h>
#include <unistd.h>

static atomic_int stop;
static volatile long bad_machine, bad_release, empty_any;

static void *hammer(void *arg)
{
    long id = (long)arg;
    char expect_machine[] = "x86_64";
    while (!stop) {
        struct utsname u;
        if (uname(&u) != 0) {
            /* uname failing at all is itself suspicious */
            bad_machine++;
            continue;
        }
        if (memcmp(u.machine, expect_machine, 7) != 0) {
            bad_machine++;
            if (bad_machine < 5)
                printf("UNAMEP BAD machine='%.12s' tid%ld\n", u.machine, id);
        }
        if (u.release[0] == 0 || u.nodename[0] == 0 || u.machine[0] == 0) {
            empty_any++;
            if (empty_any < 5)
                printf("UNAMEP EMPTY field tid%ld rel='%.8s' node='%.8s' mach='%.8s'\n",
                       id, u.release, u.nodename, u.machine);
        }
        if (u.release[0] && (u.release[0] < '0' || u.release[0] > '9')) {
            bad_release++;
            if (bad_release < 5)
                printf("UNAMEP BAD release='%.12s' tid%ld\n", u.release, id);
        }
    }
    return NULL;
}

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("UNAMEP start\n");
    pthread_t th[8];
    for (long i = 0; i < 8; i++)
        pthread_create(&th[i], NULL, hammer, (void *)i);
    struct utsname u;
    uname(&u);
    printf("UNAMEP baseline machine='%s' release='%s' node='%s'\n",
           u.machine, u.release, u.nodename);
    sleep(25);
    stop = 1;
    for (int i = 0; i < 8; i++) pthread_join(th[i], NULL);
    printf("UNAMEP-DONE bad_machine=%ld bad_release=%ld empty=%ld\n",
           bad_machine, bad_release, empty_any);
    return (bad_machine + bad_release + empty_any) > 0;
}
