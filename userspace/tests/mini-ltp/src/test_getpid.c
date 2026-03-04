/*
 * Test: getpid() / getppid()
 * 测试进程 ID 获取
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

    /* PID 必须大于 0 */
    if (pid <= 0) return 1;

    /* PPID 也必须大于 0 */
    if (ppid <= 0) return 2;

    /* fork 后测试父子关系 */
    child_pid = fork();
    if (child_pid < 0) return 3;

    if (child_pid == 0) {
        /* 子进程检查 */
        if (getppid() != pid) _exit(1);
        _exit(0);
    } else {
        wait(&status);
        if (WIFEXITED(status) && WEXITSTATUS(status) == 0) return 0;
        return 4;
    }
}
