/*
 * Rux OS Shell - musl libc 版本
 *
 * 功能：
 * - 显示提示符
 * - 读取用户输入
 * - 执行内置命令（echo, help, exit, ls, cat）
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
#include <dirent.h>
#include <fcntl.h>
#include <errno.h>

#define MAX_CMD_LEN 256
#define MAX_ARGS 16

/* 打印欢迎信息 */
static void print_welcome(void) {
    printf("\n");
    printf("========================================\n");
    printf("  Rux OS Shell v0.3 (musl libc)\n");
    printf("========================================\n");
    printf("Type 'help' for available commands\n");
    printf("\n");
}

/* 打印帮助信息 */
static void print_help(void) {
    printf("Rux OS Shell v0.3\n");
    printf("Available commands:\n");
    printf("  echo <args>  - Print arguments\n");
    printf("  help         - Show this help message\n");
    printf("  ls [dir]     - List directory contents\n");
    printf("  cat <file>   - Display file contents\n");
    printf("  time         - Show current time\n");
    printf("  pid          - Show process ID\n");
    printf("  exit         - Exit the shell\n");
    printf("  <program>    - Execute external program\n");
    printf("\n");
}

/* ls 命令 - 列出目录内容 */
static void cmd_ls(const char *dirname) {
    DIR *dir;
    struct dirent *entry;
    const char *path = dirname ? dirname : ".";

    dir = opendir(path);
    if (dir == NULL) {
        printf("ls: cannot open directory '%s': %d\n", path, errno);
        return;
    }

    printf("Contents of %s:\n", path);

    while ((entry = readdir(dir)) != NULL) {
        /* 文件类型标识 */
        char type_char = '?';
        switch (entry->d_type) {
            case DT_DIR:  type_char = 'd'; break;
            case DT_REG:  type_char = '-'; break;
            case DT_LNK:  type_char = 'l'; break;
            case DT_BLK:  type_char = 'b'; break;
            case DT_CHR:  type_char = 'c'; break;
            case DT_FIFO: type_char = 'p'; break;
            case DT_SOCK: type_char = 's'; break;
            default:      type_char = '?'; break;
        }

        printf("  %c %s\n", type_char, entry->d_name);
    }

    closedir(dir);
}

/* cat 命令 - 显示文件内容 */
static void cmd_cat(const char *filename) {
    if (filename == NULL) {
        printf("cat: missing file operand\n");
        printf("Usage: cat <filename>\n");
        return;
    }

    int fd = open(filename, O_RDONLY);
    if (fd < 0) {
        printf("cat: cannot open '%s': %d\n", filename, errno);
        return;
    }

    char buf[512];
    ssize_t bytes_read;

    while ((bytes_read = read(fd, buf, sizeof(buf))) > 0) {
        /* 写入标准输出 */
        ssize_t bytes_written = 0;
        while (bytes_written < bytes_read) {
            ssize_t n = write(STDOUT_FILENO, buf + bytes_written, bytes_read - bytes_written);
            if (n < 0) {
                printf("\ncat: write error: %d\n", errno);
                close(fd);
                return;
            }
            bytes_written += n;
        }
    }

    if (bytes_read < 0) {
        printf("\ncat: read error: %d\n", errno);
    }

    close(fd);
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

    if (strcmp(args[0], "ls") == 0) {
        cmd_ls(argc > 1 ? args[1] : NULL);
        return;
    }

    if (strcmp(args[0], "cat") == 0) {
        cmd_cat(argc > 1 ? args[1] : NULL);
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
