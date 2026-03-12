/*
 * Test: wait() / waitpid()
 * Test waiting for child processes
 */

#include <unistd.h>
#include <sys/wait.h>

int main(void)
{
    pid_t pid1, pid2;
    int status;

    /* Test wait() */
    pid1 = fork();
    if (pid1 < 0) return 1;

    if (pid1 == 0) {
        /* Child process 1 */
        _exit(10);
    }

    /* Parent process waits */
    if (wait(&status) < 0) return 2;
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 10) return 3;

    /* Test waitpid() */
    pid1 = fork();
    if (pid1 < 0) return 4;

    if (pid1 == 0) {
        /* Child process 2 */
        sleep(1);
        _exit(20);
    }

    pid2 = fork();
    if (pid2 < 0) return 5;

    if (pid2 == 0) {
        /* Child process 3 */
        _exit(30);
    }

    /* Wait for specific child process */
    if (waitpid(pid2, &status, 0) < 0) return 6;
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 30) return 7;

    /* Wait for another child process */
    if (waitpid(pid1, &status, 0) < 0) return 8;
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 20) return 9;

    return 0;
}
