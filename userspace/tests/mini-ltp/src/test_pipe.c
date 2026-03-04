/*
 * Test: pipe()
 * 测试管道通信
 */

#include <unistd.h>
#include <string.h>
#include <sys/wait.h>

int main(void)
{
    int pipefd[2];
    pid_t pid;
    char buf[32];
    const char *msg = "Hello from child!";
    int status;

    if (pipe(pipefd) < 0) return 1;

    pid = fork();
    if (pid < 0) {
        close(pipefd[0]);
        close(pipefd[1]);
        return 2;
    }

    if (pid == 0) {
        /* 子进程 - 写入管道 */
        close(pipefd[0]);
        write(pipefd[1], msg, strlen(msg));
        close(pipefd[1]);
        _exit(0);
    } else {
        /* 父进程 - 从管道读取 */
        close(pipefd[1]);
        ssize_t len = read(pipefd[0], buf, sizeof(buf) - 1);
        close(pipefd[0]);

        if (len < 0) return 3;
        buf[len] = '\0';

        wait(&status);

        if (strcmp(buf, msg) != 0) return 4;
        return 0;
    }
}
