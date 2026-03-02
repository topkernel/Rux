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
#include <sys/stat.h>

#define MAX_CMD_LEN 256
#define MAX_ARGS 16

/* ANSI 颜色代码 */
#define ANSI_RESET   "\033[0m"
#define ANSI_RED     "\033[31m"
#define ANSI_GREEN   "\033[32m"
#define ANSI_YELLOW  "\033[33m"
#define ANSI_BLUE    "\033[34m"
#define ANSI_MAGENTA "\033[35m"
#define ANSI_CYAN    "\033[36m"
#define ANSI_WHITE   "\033[37m"
#define ANSI_BOLD    "\033[1m"

/* 打印欢迎信息 */
static void print_welcome(void) {
    printf("\n");
    printf("%s========================================%s\n", ANSI_CYAN, ANSI_RESET);
    printf("%s  Rux OS Shell v0.4 (musl libc)%s\n", ANSI_BOLD ANSI_GREEN, ANSI_RESET);
    printf("%s========================================%s\n", ANSI_CYAN, ANSI_RESET);
    printf("Type '%shelp%s' for available commands\n", ANSI_YELLOW, ANSI_RESET);
    printf("\n");
}

/* 打印帮助信息 */
static void print_help(void) {
    printf("%sRux OS Shell v0.4%s\n", ANSI_BOLD ANSI_GREEN, ANSI_RESET);
    printf("\n%sAvailable commands:%s\n", ANSI_CYAN, ANSI_RESET);
    printf("  %secho%s <args>  - Print arguments\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %shelp%s         - Show this help message\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %sls%s [dir]     - List directory contents\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %scat%s <file>   - Display file contents\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %scd%s <dir>     - Change directory\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %spwd%s          - Print working directory\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %stime%s         - Show current time\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %spid%s          - Show process ID\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %sexit%s         - Exit the shell\n", ANSI_YELLOW, ANSI_RESET);
    printf("\n%sFile colors in ls:%s\n", ANSI_CYAN, ANSI_RESET);
    printf("  %sblue%s   - directory\n", ANSI_BLUE, ANSI_RESET);
    printf("  %sgreen%s  - executable\n", ANSI_GREEN, ANSI_RESET);
    printf("  %swhite%s  - regular file\n", ANSI_WHITE, ANSI_RESET);
    printf("\n%sTips:%s\n", ANSI_CYAN, ANSI_RESET);
    printf("  - Type a program name to run it\n");
    printf("  - Use Tab for completion (coming soon)\n");
    printf("\n");
}

/* 检查文件是否可执行 */
static int is_executable(const char *path, const char *name) {
    char fullpath[512];
    snprintf(fullpath, sizeof(fullpath), "%s/%s", path, name);
    struct stat st;
    if (stat(fullpath, &st) == 0) {
        /* 符号链接、普通文件都可以是可执行的 */
        if (S_ISREG(st.st_mode) || S_ISLNK(st.st_mode)) {
            return (st.st_mode & (S_IXUSR | S_IXGRP | S_IXOTH)) != 0;
        }
    }
    return 0;
}

/* ls 命令 - 列出目录内容（带颜色和多列显示） */
static void cmd_ls(const char *dirname) {
    DIR *dir;
    struct dirent *entry;
    const char *path = dirname ? dirname : ".";
    char names[256][256];  /* 最多 256 个文件，每个文件名最长 255 字符 */
    unsigned char types[256];
    int count = 0;
    int max_len = 0;

    dir = opendir(path);
    if (dir == NULL) {
        printf("ls: cannot open '%s': %s\n", path, strerror(errno));
        return;
    }

    /* 收集所有文件名并计算最大长度 */
    while ((entry = readdir(dir)) != NULL && count < 256) {
        strncpy(names[count], entry->d_name, 255);
        names[count][255] = '\0';
        types[count] = entry->d_type;
        int len = strlen(names[count]);
        if (len > max_len) max_len = len;
        count++;
    }
    closedir(dir);

    /* 计算列数（每列需要 max_len + 2 的宽度，假设终端宽度 80） */
    int col_width = max_len + 2;
    int cols = 80 / col_width;
    if (cols < 1) cols = 1;
    int rows = (count + cols - 1) / cols;

    /* 多列输出 */
    for (int row = 0; row < rows; row++) {
        for (int col = 0; col < cols; col++) {
            int idx = col * rows + row;
            if (idx >= count) break;

            const char *name = names[idx];
            unsigned char type = types[idx];

            /* 根据类型选择颜色 */
            const char *color = ANSI_WHITE;
            if (type == DT_DIR) {
                color = ANSI_BLUE;
            } else if (type == DT_REG && is_executable(path, name)) {
                color = ANSI_GREEN;
            } else if (type == DT_CHR) {
                color = ANSI_YELLOW;
            } else if (type == DT_BLK) {
                color = ANSI_CYAN;
            }

            printf("%s%-*s%s", color, col_width, name, ANSI_RESET);
        }
        printf("\n");
    }
}

/* cat 命令 - 显示文件内容 */
static void cmd_cat(const char *filename) {
    if (filename == NULL) {
        printf("%scat: missing file operand%s\n", ANSI_RED, ANSI_RESET);
        printf("Usage: cat <filename>\n");
        return;
    }

    int fd = open(filename, O_RDONLY);
    if (fd < 0) {
        printf("%scat: cannot open '%s': %s%s\n", ANSI_RED, filename, strerror(errno), ANSI_RESET);
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
                printf("\n%scat: write error: %s%s\n", ANSI_RED, strerror(errno), ANSI_RESET);
                close(fd);
                return;
            }
            bytes_written += n;
        }
    }

    if (bytes_read < 0) {
        printf("\n%scat: read error: %s%s\n", ANSI_RED, strerror(errno), ANSI_RESET);
    }

    close(fd);
}

/* 执行外部程序 */
static int run_external(const char *path, char *const argv[]) {
    /* 首先检查文件是否存在且可执行 */
    struct stat st;
    if (stat(path, &st) != 0) {
        printf("%s" "command not found: %s" "%s\n", ANSI_RED, argv[0], ANSI_RESET);
        return -1;
    }

    /* 检查是否是符号链接，如果是则获取目标文件信息 */
    if (S_ISLNK(st.st_mode)) {
        /* 符号链接：尝试获取目标文件信息 */
        /* 注意：stat() 应该自动跟随符号链接，但我们的内核可能不支持 */
        /* 所以我们直接允许执行符号链接 */
    } else if (!S_ISREG(st.st_mode)) {
        printf("%s" "not a regular file: %s" "%s\n", ANSI_RED, path, ANSI_RESET);
        return -1;
    }

    if (!(st.st_mode & (S_IXUSR | S_IXGRP | S_IXOTH))) {
        printf("%s" "permission denied: %s" "%s\n", ANSI_RED, path, ANSI_RESET);
        return -1;
    }

    pid_t pid = fork();

    if (pid < 0) {
        printf("%s" "fork failed" "%s\n", ANSI_RED, ANSI_RESET);
        return -1;
    } else if (pid == 0) {
        /* 子进程：执行程序 */
        execve(path, argv, NULL);
        /* 如果 execve 返回，说明失败了 */
        printf("%s" "execve failed: %s (%s)" "%s\n", ANSI_RED, path, strerror(errno), ANSI_RESET);
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
        printf("%sGoodbye!%s\n", ANSI_CYAN, ANSI_RESET);
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

    if (strcmp(args[0], "cd") == 0) {
        const char *dir = argc > 1 ? args[1] : "/";
        if (chdir(dir) != 0) {
            printf("%scd: cannot change to '%s': %s%s\n", ANSI_RED, dir, strerror(errno), ANSI_RESET);
        }
        return;
    }

    if (strcmp(args[0], "pwd") == 0) {
        char cwd[256];
        if (getcwd(cwd, sizeof(cwd)) != NULL) {
            printf("%s%s%s\n", ANSI_CYAN, cwd, ANSI_RESET);
        } else {
            printf("%spwd: cannot get current directory: %s%s\n", ANSI_RED, strerror(errno), ANSI_RESET);
        }
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

    (void)argc;
    (void)argv;

    print_welcome();

    /* NOTE: malloc/free is currently broken due to musl libc issue.
     * The crash occurs in free() at address 0x0, which is musl's a_crash()
     * function indicating an assertion failure (likely ctx.secret != area->check).
     * Investigation showed all metadata is correctly initialized, but musl's
     * internal state has an inconsistency we cannot detect from user space.
     * Avoid using dynamic memory allocation until this is resolved.
     */

    while (1) {
        /* 显示彩色提示符 */
        printf("%srux%s>%s ", ANSI_GREEN, ANSI_CYAN, ANSI_RESET);
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
