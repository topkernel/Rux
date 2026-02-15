/*
 * Rux OS Shell - musl libc 版本
 *
 * 功能：
 * - 显示提示符
 * - 读取用户输入
 * - 执行内置命令（echo, help, exit）
 * - 执行外部程序（通过 fork + execve + wait）
 *
 * 使用 musl libc 提供的标准 C 库函数
 */

#include <unistd.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <sys/time.h>

#define MAX_CMD_LEN 256
#define MAX_ARGS 16

/* 打印欢迎信息 */
static void print_welcome(void) {
    printf("\n");
    printf("========================================\n");
    printf("  Rux OS Shell v0.2 (musl libc)\n");
    printf("========================================\n");
    printf("Type 'help' for available commands\n");
    printf("\n");
}

/* 打印帮助信息 */
static void print_help(void) {
    printf("Rux OS Shell v0.2\n");
    printf("Available commands:\n");
    printf("  echo <args>  - Print arguments\n");
    printf("  help         - Show this help message\n");
    printf("  time         - Show current time\n");
    printf("  exit         - Exit the shell\n");
    printf("  <program>    - Execute external program\n");
    printf("\n");
}

/* 执行外部程序 */
static int run_external(const char *path, char *const argv[]) {
    pid_t pid = fork();

    if (pid < 0) {
        printf("fork failed\n");
        return -1;
    } else if (pid == 0) {
        /* 子进程：执行程序 */
        execve(path, argv, NULL);
        /* 如果 execve 返回，说明失败了 */
        printf("execve failed: %s\n", path);
        exit(1);
    } else {
        /* 父进程：等待子进程结束 */
        int status;
        waitpid(pid, &status, 0);
        return 0;
    }
}

/* 解析并执行命令 */
static void execute_command(char *cmd) {
    char *args[MAX_ARGS];
    int argc = 0;

    /* 跳过前导空格 */
    while (*cmd == ' ' || *cmd == '\t') cmd++;
    if (*cmd == '\0') return;

    /* 解析参数 */
    char *token = strtok(cmd, " \t\n");
    while (token != NULL && argc < MAX_ARGS - 1) {
        args[argc++] = token;
        token = strtok(NULL, " \t\n");
    }
    args[argc] = NULL;

    if (argc == 0) return;

    /* 处理内置命令 */
    if (strcmp(args[0], "echo") == 0) {
        for (int i = 1; i < argc; i++) {
            printf("%s", args[i]);
            if (i < argc - 1) printf(" ");
        }
        printf("\n");
        return;
    }

    if (strcmp(args[0], "help") == 0) {
        print_help();
        return;
    }

    if (strcmp(args[0], "exit") == 0 || strcmp(args[0], "quit") == 0) {
        printf("Goodbye!\n");
        exit(0);
    }

    if (strcmp(args[0], "time") == 0) {
        struct timeval tv;
        gettimeofday(&tv, NULL);
        printf("Current time: %ld.%06ld seconds since epoch\n", tv.tv_sec, tv.tv_usec);
        return;
    }

    if (strcmp(args[0], "pid") == 0) {
        printf("PID: %d\n", getpid());
        printf("PPID: %d\n", getppid());
        return;
    }

    /* 执行外部程序 */
    char path[256];

    if (args[0][0] == '/' || args[0][0] == '.') {
        /* 绝对路径或相对路径 */
        strncpy(path, args[0], sizeof(path) - 1);
    } else {
        /* 在 /bin 中查找 */
        snprintf(path, sizeof(path), "/bin/%s", args[0]);
    }

    run_external(path, args);
}

/* 主函数 */
int main(int argc, char *argv[]) {
    char cmd[MAX_CMD_LEN];

    print_welcome();

    while (1) {
        printf("rux> ");
        fflush(stdout);

        if (fgets(cmd, sizeof(cmd), stdin) == NULL) {
            break;
        }

        /* 移除换行符 */
        size_t len = strlen(cmd);
        if (len > 0 && cmd[len - 1] == '\n') {
            cmd[len - 1] = '\0';
        }

        execute_command(cmd);
    }

    return 0;
}
