/*
 * Rux OS Shell - musl libc version
 *
 * Features:
 * - Display prompt
 * - Read user input with line editing
 * - Command history (up/down arrows)
 * - Tab completion
 * - Backspace support
 * - Execute built-in commands (echo, help, exit, ls, cat)
 * - Execute external programs (via fork + execve + wait)
 *
 * Uses standard C library functions provided by musl libc
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
#include <termios.h>

#define MAX_CMD_LEN 256
#define MAX_ARGS 16
#define MAX_HISTORY 64
#define MAX_COMPLETIONS 64

/* ANSI color codes */
#define ANSI_RESET   "\033[0m"
#define ANSI_RED     "\033[31m"
#define ANSI_GREEN   "\033[32m"
#define ANSI_YELLOW  "\033[33m"
#define ANSI_BLUE    "\033[34m"
#define ANSI_MAGENTA "\033[35m"
#define ANSI_CYAN    "\033[36m"
#define ANSI_WHITE   "\033[37m"
#define ANSI_BOLD    "\033[1m"

/* Key codes */
#define KEY_BACKSPACE 0x7F
#define KEY_BACKSPACE2 0x08
#define KEY_TAB       0x09
#define KEY_ENTER     0x0A
#define KEY_ESC       0x1B

/* Command history */
static char history[MAX_HISTORY][MAX_CMD_LEN];
static int history_count = 0;
static int history_index = 0;

/* Original terminal settings */
static struct termios orig_termios;

/* Enable raw mode for character-by-character input */
static void enable_raw_mode(void) {
    struct termios raw;
    tcgetattr(STDIN_FILENO, &orig_termios);
    raw = orig_termios;
    raw.c_lflag &= ~(ECHO | ICANON);  /* Disable echo and line buffering */
    tcsetattr(STDIN_FILENO, TCSAFLUSH, &raw);
}

/* Restore original terminal settings */
static void disable_raw_mode(void) {
    tcsetattr(STDIN_FILENO, TCSAFLUSH, &orig_termios);
}

/* Add command to history */
static void add_to_history(const char *cmd) {
    if (cmd[0] == '\0') return;  /* Don't add empty commands */

    /* Don't add duplicate of last command */
    if (history_count > 0 && strcmp(history[history_count - 1], cmd) == 0) {
        return;
    }

    if (history_count < MAX_HISTORY) {
        strncpy(history[history_count], cmd, MAX_CMD_LEN - 1);
        history[history_count][MAX_CMD_LEN - 1] = '\0';
        history_count++;
    } else {
        /* Shift history left */
        for (int i = 0; i < MAX_HISTORY - 1; i++) {
            strcpy(history[i], history[i + 1]);
        }
        strncpy(history[MAX_HISTORY - 1], cmd, MAX_CMD_LEN - 1);
        history[MAX_HISTORY - 1][MAX_CMD_LEN - 1] = '\0';
    }
}

/* Clear current line and redraw prompt + buffer */
static void redraw_line(const char *prompt, const char *buf, int cursor) {
    (void)cursor;  /* Currently not used for cursor positioning */
    /* Clear line: \r + clear to end of line */
    write(STDOUT_FILENO, "\r\033[K", 4);
    /* Write prompt and buffer */
    write(STDOUT_FILENO, prompt, strlen(prompt));
    write(STDOUT_FILENO, buf, strlen(buf));
}

/* Find completions for a partial command/file */
static int find_completions(const char *partial, char completions[MAX_COMPLETIONS][MAX_CMD_LEN]) {
    int count = 0;
    int partial_len = strlen(partial);

    if (partial_len == 0) return 0;

    /* Find last component (for paths) */
    const char *last_slash = strrchr(partial, '/');
    const char *name_part = last_slash ? last_slash + 1 : partial;

    /* Determine directory to search */
    char dir_path[MAX_CMD_LEN];
    if (last_slash) {
        int dir_len = last_slash - partial;
        if (dir_len == 0) {
            strcpy(dir_path, "/");
        } else {
            strncpy(dir_path, partial, dir_len);
            dir_path[dir_len] = '\0';
        }
    } else {
        strcpy(dir_path, ".");
    }

    DIR *dir = opendir(dir_path);
    if (!dir) return 0;

    struct dirent *entry;
    while ((entry = readdir(dir)) != NULL && count < MAX_COMPLETIONS) {
        /* Skip hidden files unless partial starts with . */
        if (entry->d_name[0] == '.' && name_part[0] != '.') continue;

        /* Check if name matches partial */
        if (strncmp(entry->d_name, name_part, strlen(name_part)) == 0) {
            /* Build full completion */
            if (last_slash) {
                snprintf(completions[count], MAX_CMD_LEN, "%.*s%s",
                         (int)(last_slash - partial + 1), partial, entry->d_name);
            } else {
                strncpy(completions[count], entry->d_name, MAX_CMD_LEN - 1);
                completions[count][MAX_CMD_LEN - 1] = '\0';
            }
            count++;
        }
    }
    closedir(dir);

    return count;
}

/* Find common prefix among completions */
static int find_common_prefix(char completions[MAX_COMPLETIONS][MAX_CMD_LEN], int count,
                              char *prefix, int prefix_size) {
    if (count == 0) return 0;

    strncpy(prefix, completions[0], prefix_size - 1);
    prefix[prefix_size - 1] = '\0';
    int prefix_len = strlen(prefix);

    for (int i = 1; i < count && prefix_len > 0; i++) {
        int j;
        for (j = 0; j < prefix_len && completions[i][j] == prefix[j]; j++);
        prefix_len = j;
        prefix[j] = '\0';
    }

    return prefix_len;
}

/* Print welcome message */
static void print_welcome(void) {
    printf("\n");
    printf("%s========================================%s\n", ANSI_CYAN, ANSI_RESET);
    printf("%s  Rux OS Shell v0.5 (musl libc)%s\n", ANSI_BOLD ANSI_GREEN, ANSI_RESET);
    printf("%s========================================%s\n", ANSI_CYAN, ANSI_RESET);
    printf("Type '%shelp%s' for available commands\n", ANSI_YELLOW, ANSI_RESET);
    printf("\n");
}

/* Print help message */
static void print_help(void) {
    printf("%sRux OS Shell v0.5%s\n", ANSI_BOLD ANSI_GREEN, ANSI_RESET);
    printf("\n%sAvailable commands:%s\n", ANSI_CYAN, ANSI_RESET);
    printf("  %secho%s <args>  - Print arguments\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %shelp%s         - Show this help message\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %sls%s [dir]     - List directory contents\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %scat%s <file>   - Display file contents\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %scd%s <dir>     - Change directory\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %spwd%s          - Print working directory\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %stime%s         - Show current time\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %spid%s          - Show process ID\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %shistory%s      - Show command history\n", ANSI_YELLOW, ANSI_RESET);
    printf("  %sexit%s         - Exit the shell\n", ANSI_YELLOW, ANSI_RESET);
    printf("\n%sFile colors in ls:%s\n", ANSI_CYAN, ANSI_RESET);
    printf("  %sblue%s   - directory\n", ANSI_BLUE, ANSI_RESET);
    printf("  %sgreen%s  - executable\n", ANSI_GREEN, ANSI_RESET);
    printf("  %swhite%s  - regular file\n", ANSI_WHITE, ANSI_RESET);
    printf("\n%sLine editing:%s\n", ANSI_CYAN, ANSI_RESET);
    printf("  Tab         - Auto-complete\n");
    printf("  Up/Down     - Command history\n");
    printf("  Backspace   - Delete character\n");
    printf("\n");
}

/* Check if file is executable */
static int is_executable(const char *path, const char *name) {
    char fullpath[512];
    snprintf(fullpath, sizeof(fullpath), "%s/%s", path, name);
    struct stat st;
    if (stat(fullpath, &st) == 0) {
        /* Symbolic links and regular files can both be executable */
        if (S_ISREG(st.st_mode) || S_ISLNK(st.st_mode)) {
            return (st.st_mode & (S_IXUSR | S_IXGRP | S_IXOTH)) != 0;
        }
    }
    return 0;
}

/* ls command - list directory contents (with colors and multi-column display) */
static void cmd_ls(const char *dirname) {
    DIR *dir;
    struct dirent *entry;
    const char *path = dirname ? dirname : ".";
    char names[256][256];  /* Max 256 files, each filename up to 255 chars */
    unsigned char types[256];
    int count = 0;
    int max_len = 0;

    dir = opendir(path);
    if (dir == NULL) {
        printf("ls: cannot open '%s': %s\n", path, strerror(errno));
        return;
    }

    /* Collect all filenames and calculate maximum length */
    while ((entry = readdir(dir)) != NULL && count < 256) {
        strncpy(names[count], entry->d_name, 255);
        names[count][255] = '\0';
        types[count] = entry->d_type;
        int len = strlen(names[count]);
        if (len > max_len) max_len = len;
        count++;
    }
    closedir(dir);

    /* Calculate number of columns (each column needs max_len + 2 width, assume 80 char terminal) */
    int col_width = max_len + 2;
    int cols = 80 / col_width;
    if (cols < 1) cols = 1;
    int rows = (count + cols - 1) / cols;

    /* Multi-column output */
    for (int row = 0; row < rows; row++) {
        for (int col = 0; col < cols; col++) {
            int idx = col * rows + row;
            if (idx >= count) break;

            const char *name = names[idx];
            unsigned char type = types[idx];

            /* Select color based on type */
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

/* cat command - display file contents */
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
        /* Write to standard output */
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

/* Execute external program */
static int run_external(const char *path, char *const argv[]) {
    /* First check if file exists and is executable */
    struct stat st;
    if (stat(path, &st) != 0) {
        printf("%s" "command not found: %s" "%s\n", ANSI_RED, argv[0], ANSI_RESET);
        return -1;
    }

    /* Check if it's a symbolic link, if so get target file info */
    if (S_ISLNK(st.st_mode)) {
        /* Symbolic link: try to get target file info */
        /* Note: stat() should automatically follow symbolic links, but our kernel may not support it */
        /* So we directly allow executing symbolic links */
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
        /* Child process: execute program */
        execve(path, argv, NULL);
        /* If execve returns, it failed */
        printf("%s" "execve failed: %s (%s)" "%s\n", ANSI_RED, path, strerror(errno), ANSI_RESET);
        exit(1);
    } else {
        /* Parent process: wait for child to finish */
        int status;
        waitpid(pid, &status, 0);
        return 0;
    }
}

/* Parse and execute command with redirection support */
static void execute_command(char *cmd) {
    char *args[MAX_ARGS];
    int argc = 0;
    char *redirect_out = NULL;   /* Output redirect file */
    char *redirect_in = NULL;    /* Input redirect file */
    int redirect_append = 0;     /* Append mode for output */
    int saved_stdout = -1;       /* Saved stdout fd */
    int saved_stdin = -1;        /* Saved stdin fd */

    /* Skip leading whitespace */
    while (*cmd == ' ' || *cmd == '\t') cmd++;
    if (*cmd == '\0') return;

    /* Make a copy of command for parsing (strtok modifies it) */
    static char cmd_copy[MAX_CMD_LEN];
    strncpy(cmd_copy, cmd, MAX_CMD_LEN - 1);
    cmd_copy[MAX_CMD_LEN - 1] = '\0';

    /* Parse arguments, handling redirection operators */
    char *token = strtok(cmd_copy, " \t\n");
    while (token != NULL && argc < MAX_ARGS - 1) {
        if (strcmp(token, ">") == 0) {
            /* Output redirect (truncate) */
            token = strtok(NULL, " \t\n");
            if (token != NULL) {
                redirect_out = token;
                redirect_append = 0;
            }
        } else if (strcmp(token, ">>") == 0) {
            /* Output redirect (append) */
            token = strtok(NULL, " \t\n");
            if (token != NULL) {
                redirect_out = token;
                redirect_append = 1;
            }
        } else if (strcmp(token, "<") == 0) {
            /* Input redirect */
            token = strtok(NULL, " \t\n");
            if (token != NULL) {
                redirect_in = token;
            }
        } else {
            /* Regular argument */
            args[argc++] = token;
        }
        token = strtok(NULL, " \t\n");
    }
    args[argc] = NULL;

    if (argc == 0) return;

    /* Handle output redirection */
    if (redirect_out != NULL) {
        int flags = O_WRONLY | O_CREAT;
        if (redirect_append) {
            flags |= O_APPEND;
        } else {
            flags |= O_TRUNC;
        }
        int fd = open(redirect_out, flags, 0644);
        if (fd < 0) {
            printf("%s" "cannot open %s: %s" "%s\n", ANSI_RED, redirect_out, strerror(errno), ANSI_RESET);
            return;
        }
        /* Save stdout and redirect */
        saved_stdout = dup(STDOUT_FILENO);
        if (saved_stdout < 0) {
            close(fd);
            printf("%s" "dup failed" "%s\n", ANSI_RED, ANSI_RESET);
            return;
        }
        if (dup2(fd, STDOUT_FILENO) < 0) {
            close(fd);
            close(saved_stdout);
            printf("%s" "redirect failed" "%s\n", ANSI_RED, ANSI_RESET);
            return;
        }
        close(fd);
    }

    /* Handle input redirection */
    if (redirect_in != NULL) {
        int fd = open(redirect_in, O_RDONLY);
        if (fd < 0) {
            printf("%s" "cannot open %s: %s" "%s\n", ANSI_RED, redirect_in, strerror(errno), ANSI_RESET);
            /* Restore stdout if it was redirected */
            if (saved_stdout >= 0) {
                dup2(saved_stdout, STDOUT_FILENO);
                close(saved_stdout);
            }
            return;
        }
        /* Save stdin and redirect */
        saved_stdin = dup(STDIN_FILENO);
        if (saved_stdin < 0) {
            close(fd);
            /* Restore stdout if it was redirected */
            if (saved_stdout >= 0) {
                dup2(saved_stdout, STDOUT_FILENO);
                close(saved_stdout);
            }
            printf("%s" "dup failed" "%s\n", ANSI_RED, ANSI_RESET);
            return;
        }
        if (dup2(fd, STDIN_FILENO) < 0) {
            close(fd);
            close(saved_stdin);
            /* Restore stdout if it was redirected */
            if (saved_stdout >= 0) {
                dup2(saved_stdout, STDOUT_FILENO);
                close(saved_stdout);
            }
            printf("%s" "redirect failed" "%s\n", ANSI_RED, ANSI_RESET);
            return;
        }
        close(fd);
    }

    /* Define cleanup macro for redirection restoration */
    #define RESTORE_REDIR() do { \
        if (saved_stdout >= 0) { \
            dup2(saved_stdout, STDOUT_FILENO); \
            close(saved_stdout); \
        } \
        if (saved_stdin >= 0) { \
            dup2(saved_stdin, STDIN_FILENO); \
            close(saved_stdin); \
        } \
    } while(0)

    /* Handle built-in commands */
    if (strcmp(args[0], "echo") == 0) {
        for (int i = 1; i < argc; i++) {
            printf("%s", args[i]);
            if (i < argc - 1) printf(" ");
        }
        printf("\n");
        RESTORE_REDIR();
        return;
    }

    if (strcmp(args[0], "help") == 0) {
        print_help();
        RESTORE_REDIR();
        return;
    }

    if (strcmp(args[0], "exit") == 0 || strcmp(args[0], "quit") == 0) {
        printf("%sGoodbye!%s\n", ANSI_CYAN, ANSI_RESET);
        RESTORE_REDIR();
        disable_raw_mode();
        exit(0);
    }

    if (strcmp(args[0], "time") == 0) {
        struct timeval tv;
        gettimeofday(&tv, NULL);
        printf("Current time: %ld.%06ld seconds since epoch\n", tv.tv_sec, tv.tv_usec);
        RESTORE_REDIR();
        return;
    }

    if (strcmp(args[0], "pid") == 0) {
        printf("PID: %d\n", getpid());
        printf("PPID: %d\n", getppid());
        RESTORE_REDIR();
        return;
    }

    if (strcmp(args[0], "ls") == 0) {
        cmd_ls(argc > 1 ? args[1] : NULL);
        RESTORE_REDIR();
        return;
    }

    if (strcmp(args[0], "cat") == 0) {
        cmd_cat(argc > 1 ? args[1] : NULL);
        RESTORE_REDIR();
        return;
    }

    if (strcmp(args[0], "cd") == 0) {
        const char *dir = argc > 1 ? args[1] : "/";
        if (chdir(dir) != 0) {
            printf("%scd: cannot change to '%s': %s%s\n", ANSI_RED, dir, strerror(errno), ANSI_RESET);
        }
        RESTORE_REDIR();
        return;
    }

    if (strcmp(args[0], "pwd") == 0) {
        char cwd[256];
        if (getcwd(cwd, sizeof(cwd)) != NULL) {
            printf("%s%s%s\n", ANSI_CYAN, cwd, ANSI_RESET);
        } else {
            printf("%spwd: cannot get current directory: %s%s\n", ANSI_RED, strerror(errno), ANSI_RESET);
        }
        RESTORE_REDIR();
        return;
    }

    if (strcmp(args[0], "history") == 0) {
        for (int i = 0; i < history_count; i++) {
            printf("%4d  %s\n", i + 1, history[i]);
        }
        RESTORE_REDIR();
        return;
    }

    /* Execute external program */
    char path[256];

    if (args[0][0] == '/' || args[0][0] == '.') {
        /* Absolute path or relative path */
        strncpy(path, args[0], sizeof(path) - 1);
    } else {
        /* Search in /bin */
        snprintf(path, sizeof(path), "/bin/%s", args[0]);
    }

    run_external(path, args);
    RESTORE_REDIR();
}

/* Read a line with editing support */
static int read_line(char *buf, int max_len) {
    int len = 0;
    int hist_idx = history_count;
    char c;
    char prompt[] = "\033[31mroot\033[0m# ";

    /* Display initial prompt */
    write(STDOUT_FILENO, prompt, strlen(prompt));

    while (1) {
        if (read(STDIN_FILENO, &c, 1) != 1) {
            break;
        }

        if (c == KEY_ENTER) {
            /* Enter: finish line */
            buf[len] = '\0';
            write(STDOUT_FILENO, "\n", 1);
            return len;
        }
        else if (c == KEY_BACKSPACE || c == KEY_BACKSPACE2) {
            /* Backspace: delete last character */
            if (len > 0) {
                len--;
                buf[len] = '\0';
                redraw_line(prompt, buf, len);
            }
        }
        else if (c == KEY_TAB) {
            /* Tab: auto-complete */
            if (len > 0) {
                /* Find the word to complete */
                int word_start = len - 1;
                while (word_start > 0 && buf[word_start - 1] != ' ' && buf[word_start - 1] != '/') {
                    word_start--;
                }

                char word[MAX_CMD_LEN];
                strncpy(word, buf + word_start, len - word_start);
                word[len - word_start] = '\0';

                char completions[MAX_COMPLETIONS][MAX_CMD_LEN];
                int count = find_completions(word, completions);

                if (count == 1) {
                    /* Single match: complete it */
                    int word_len = strlen(word);
                    int completion_len = strlen(completions[0]);

                    /* Check if it's a directory */
                    char full_path[MAX_CMD_LEN * 2];
                    struct stat st;
                    if (word[0] == '/' || (word[0] == '.' && word[1] == '/')) {
                        snprintf(full_path, sizeof(full_path), "%s", completions[0]);
                    } else {
                        snprintf(full_path, sizeof(full_path), "./%s", completions[0]);
                    }

                    /* Append / if directory */
                    int is_dir = (stat(full_path, &st) == 0 && S_ISDIR(st.st_mode));

                    /* Replace word with completion */
                    len = word_start + completion_len;
                    strcpy(buf + word_start, completions[0]);
                    if (is_dir && len < max_len - 1) {
                        buf[len] = '/';
                        len++;
                        buf[len] = '\0';
                    }

                    redraw_line(prompt, buf, len);
                }
                else if (count > 1) {
                    /* Multiple matches: find common prefix and show options */
                    char common[MAX_CMD_LEN];
                    int common_len = find_common_prefix(completions, count, common, sizeof(common));

                    if (common_len > 0) {
                        int word_len = strlen(word);
                        if (common_len > word_len) {
                            /* Extend with common prefix */
                            len = word_start + common_len;
                            strncpy(buf + word_start, common, common_len);
                            buf[len] = '\0';
                            redraw_line(prompt, buf, len);
                        }
                    }

                    /* Show all completions */
                    write(STDOUT_FILENO, "\n", 1);
                    for (int i = 0; i < count; i++) {
                        write(STDOUT_FILENO, completions[i], strlen(completions[i]));
                        write(STDOUT_FILENO, "  ", 2);
                    }
                    write(STDOUT_FILENO, "\n", 1);
                    redraw_line(prompt, buf, len);
                }
            }
        }
        else if (c == KEY_ESC) {
            /* Escape sequence (arrow keys) */
            char seq[2];
            if (read(STDIN_FILENO, &seq[0], 1) != 1) continue;
            if (read(STDIN_FILENO, &seq[1], 1) != 1) continue;

            if (seq[0] == '[') {
                if (seq[1] == 'A') {
                    /* Up arrow: previous history */
                    if (hist_idx > 0) {
                        hist_idx--;
                        strcpy(buf, history[hist_idx]);
                        len = strlen(buf);
                        redraw_line(prompt, buf, len);
                    }
                }
                else if (seq[1] == 'B') {
                    /* Down arrow: next history */
                    if (hist_idx < history_count - 1) {
                        hist_idx++;
                        strcpy(buf, history[hist_idx]);
                        len = strlen(buf);
                        redraw_line(prompt, buf, len);
                    } else if (hist_idx == history_count - 1) {
                        /* At end: clear line */
                        hist_idx = history_count;
                        buf[0] = '\0';
                        len = 0;
                        redraw_line(prompt, buf, len);
                    }
                }
                /* Ignore other escape sequences (left/right arrows, etc.) */
            }
        }
        else if (c >= ' ' && c <= '~' && len < max_len - 1) {
            /* Printable character: add to buffer and echo */
            buf[len] = c;
            len++;
            buf[len] = '\0';
            write(STDOUT_FILENO, &c, 1);  /* Manual echo */
        }
        /* Ignore other characters (Ctrl+C, etc.) */
    }

    buf[len] = '\0';
    return len;
}

/* Main function */
int main(int argc, char *argv[]) {
    char cmd[MAX_CMD_LEN];

    (void)argc;
    (void)argv;

    print_welcome();

    /* Enable raw mode for character input */
    enable_raw_mode();

    while (1) {
        if (read_line(cmd, sizeof(cmd)) < 0) {
            break;
        }

        /* Add to history and execute */
        add_to_history(cmd);
        execute_command(cmd);
    }

    disable_raw_mode();
    return 0;
}
