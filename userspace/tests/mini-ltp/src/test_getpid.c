/*
 * Test: getpid() / getppid()
 * Test process ID retrieval
 */

#include <unistd.h>
#include <sys/wait.h>

int main(void)
{
    pid_t pid, ppid;
    pid_t child_pid;
    int status;

    pid = getpid();
    ppid = getppid();

    /* PID must be greater than 0 */
    if (pid <= 0) return 1;

    /* PPID must also be greater than 0 */
    if (ppid <= 0) return 2;

    /* Test parent-child relationship after fork */
    child_pid = fork();
    if (child_pid < 0) return 3;

    if (child_pid == 0) {
        /* Child process check */
        if (getppid() != pid) _exit(1);
        _exit(0);
    } else {
        wait(&status);
        if (WIFEXITED(status) && WEXITSTATUS(status) == 0) return 0;
        return 4;
    }
}
