/*
 * Test: fork()
 * 测试进程创建
 */

#include <unistd.h>
#include <sys/wait.h>

int main(void)
{
    pid_t pid;
    int status;

    pid = fork();

    if (pid < 0) {
        return 1;  /* fork 失败 */
    } else if (pid == 0) {
        /* 子进程 */
        _exit(42);
    } else {
        /* 父进程 */
        wait(&status);
        if (WIFEXITED(status) && WEXITSTATUS(status) == 42) {
            return 0;  /* 测试通过 */
        }
        return 1;  /* 测试失败 */
    }
}
