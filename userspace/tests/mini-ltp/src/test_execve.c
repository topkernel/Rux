/*
 * Test: execve()
 * 测试程序执行
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
        /* 子进程执行 /bin/true */
        execve("/bin/true", argv, envp);
        /* 如果 execve 失败，退出 */
        _exit(127);
    }

    /* 父进程等待 */
    wait(&status);

    /* /bin/true 应该返回 0 */
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) return 2;

    return 0;
}
