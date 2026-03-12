/*
 * Test: execve()
 * Test program execution
 */

#include <unistd.h>
#include <sys/wait.h>

int main(void)
{
    pid_t pid;
    int status;
    char *argv[] = {"/bin/true", NULL};
    char *envp[] = {NULL};

    pid = fork();
    if (pid < 0) return 1;

    if (pid == 0) {
        /* Child process executes /bin/true */
        execve("/bin/true", argv, envp);
        /* If execve fails, exit */
        _exit(127);
    }

    /* Parent process waits */
    wait(&status);

    /* /bin/true should return 0 */
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) return 2;

    return 0;
}
