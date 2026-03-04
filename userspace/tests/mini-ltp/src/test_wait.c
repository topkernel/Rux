/*
 * Test: wait() / waitpid()
 * 测试等待子进程
 */

#include <unistd.h>
#include <sys/wait.h>

int main(void)
{
    pid_t pid1, pid2;
    int status;

    /* 测试 wait() */
    pid1 = fork();
    if (pid1 < 0) return 1;

    if (pid1 == 0) {
        /* 子进程 1 */
        _exit(10);
    }

    /* 父进程等待 */
    if (wait(&status) < 0) return 2;
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 10) return 3;

    /* 测试 waitpid() */
    pid1 = fork();
    if (pid1 < 0) return 4;

    if (pid1 == 0) {
        /* 子进程 2 */
        sleep(1);
        _exit(20);
    }

    pid2 = fork();
    if (pid2 < 0) return 5;

    if (pid2 == 0) {
        /* 子进程 3 */
        _exit(30);
    }

    /* 等待特定子进程 */
    if (waitpid(pid2, &status, 0) < 0) return 6;
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 30) return 7;

    /* 等待另一个子进程 */
    if (waitpid(pid1, &status, 0) < 0) return 8;
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 20) return 9;

    return 0;
}
