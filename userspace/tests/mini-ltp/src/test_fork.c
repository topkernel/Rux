/*
 * Test: fork()
 * Test process creation
 */

#include <unistd.h>
#include <sys/wait.h>

int main(void)
{
    pid_t pid;
    int status;

    pid = fork();

    if (pid < 0) {
        return 1;  /* fork failed */
    } else if (pid == 0) {
        /* Child process */
        _exit(42);
    } else {
        /* Parent process */
        wait(&status);
        if (WIFEXITED(status) && WEXITSTATUS(status) == 42) {
            return 0;  /* Test passed */
        }
        return 1;  /* Test failed */
    }
}
