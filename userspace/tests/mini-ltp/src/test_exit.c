/*
 * Test: exit() / _exit()
 * Test process exit
 */

#include <unistd.h>
#include <stdlib.h>
#include <sys/wait.h>

int main(void)
{
    pid_t pid;
    int status;

    /* Test exit() */
    pid = fork();
    if (pid < 0) return 1;

    if (pid == 0) {
        exit(123);
    }

    wait(&status);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 123) return 2;

    /* Test _exit() */
    pid = fork();
    if (pid < 0) return 3;

    if (pid == 0) {
        _exit(45);
    }

    wait(&status);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 45) return 4;

    return 0;
}
