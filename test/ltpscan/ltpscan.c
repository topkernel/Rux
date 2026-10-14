/*
 * ltpscan.c — LTP chunk sweep runner for the Rux kernel (runs as PID 1).
 *
 * Reads /test/linux-ltp/chunk.txt (one test name per line), runs each
 * /test/linux-ltp/testcases/bin/<name> with a wall-clock timeout
 * (SIGTERM at 30s, SIGKILL at 40s), captures the last 220 bytes of the
 * child's combined output, and prints one parseable line per test to
 * /dev/console:
 *
 *     RES|<name>|<rc>|<output-tail-flattened>
 *
 * rc is the waitpid status: WEXITSTATUS for normal exit, 128+sig for
 * signal death (so 158 = SIGKILL from the wall-clock kill = TIMEOUT).
 * Sentinels SCAN_BEGIN / SCAN_DONE bracket the sweep; PID 1 parks in a
 * sleep loop afterwards so the kernel never sees init exit.
 *
 * Build: riscv64-linux-gnu-gcc -static -O2 -o ltpscan test/ltpscan/ltpscan.c
 */
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define LTP_BIN "/test/linux-ltp/testcases/bin"
#define CHUNK_FILE "/test/linux-ltp/chunk.txt"
#define OUT_FILE "/tmp/ltpscan.out"
#define TERM_AFTER_MS 30000
#define KILL_AFTER_MS 40000
#define TAIL_BYTES 220

static void flush_tail(int fd, const char *name, int rc)
{
    char buf[TAIL_BYTES + 1];
    char flat[TAIL_BYTES * 2 + 1];
    off_t sz = lseek(fd, 0, SEEK_END);
    ssize_t want = TAIL_BYTES;
    ssize_t got = 0;
    size_t o = 0;

    if (sz > 0) {
        if ((off_t)want > sz)
            want = (ssize_t)sz;
        if (lseek(fd, sz - want, SEEK_SET) < 0)
            want = 0;
        if (want > 0)
            got = read(fd, buf, want);
    }
    for (ssize_t i = 0; i < got; i++) {
        char c = buf[i];
        /* Flatten control chars so the RES line stays on one console line */
        if (c == '\n' || c == '\r' || c == '\t')
            flat[o++] = ' ';
        else if ((unsigned char)c >= 0x20 || c == '\0')
            flat[o++] = c;
        if (o >= sizeof(flat) - 1)
            break;
    }
    /* Drop trailing spaces from stripped newlines */
    while (o > 0 && flat[o - 1] == ' ')
        o--;
    flat[o] = '\0';
    printf("RES|%s|%d|%s\n", name, rc, flat);
    fflush(stdout);
}

static int run_one(const char *name)
{
    char binpath[512];
    pid_t pid;
    int outfd, devnull;
    struct timespec ts = { 0, 200 * 1000 * 1000 };
    int status = 0;
    uint64_t elapsed_ms = 0;

    snprintf(binpath, sizeof(binpath), "%s/%s", LTP_BIN, name);

    outfd = open(OUT_FILE, O_CREAT | O_TRUNC | O_RDWR, 0644);
    if (outfd < 0) {
        printf("RES|%s|252|open-out-failed\n", name);
        fflush(stdout);
        return -1;
    }
    devnull = open("/dev/null", O_RDONLY);

    pid = fork();
    if (pid < 0) {
        printf("RES|%s|253|fork-failed\n", name);
        fflush(stdout);
        close(outfd);
        if (devnull >= 0)
            close(devnull);
        return -1;
    }
    if (pid == 0) {
        /* child */
        dup2(devnull >= 0 ? devnull : outfd, 0);
        dup2(outfd, 1);
        dup2(outfd, 2);
        execl(binpath, name, (char *)NULL);
        /* exec failed: report via the output file */
        dprintf(1, "exec-failed: %s: %s\n", binpath, strerror(errno));
        _exit(127);
    }
    if (devnull >= 0)
        close(devnull);

    for (;;) {
        pid_t r = waitpid(pid, &status, WNOHANG);
        if (r == pid)
            break;
        if (r < 0 && errno != EINTR) {
            status = 254;
            break;
        }
        nanosleep(&ts, NULL);
        elapsed_ms += 200;
        if (elapsed_ms == TERM_AFTER_MS)
            kill(pid, SIGTERM);
        else if (elapsed_ms >= KILL_AFTER_MS) {
            kill(pid, SIGKILL);
            /* Reap next iteration; loop again until waitpid succeeds */
            for (;;) {
                r = waitpid(pid, &status, WNOHANG);
                if (r == pid)
                    goto reaped;
                if (r < 0 && errno != EINTR) {
                    status = 254;
                    goto reaped;
                }
                nanosleep(&ts, NULL);
            }
        }
    }
reaped:
    close(outfd); /* child is gone; its dup is closed too */
    {
        int rfd = open(OUT_FILE, O_RDONLY);
        int rc;
        if (WIFSIGNALED(status))
            rc = 128 + WTERMSIG(status);
        else if (WIFEXITED(status))
            rc = WEXITSTATUS(status);
        else
            rc = 255;
        if (rfd >= 0) {
            flush_tail(rfd, name, rc);
            close(rfd);
        } else {
            printf("RES|%s|%d|\n", name, rc);
            fflush(stdout);
        }
    }
    return 0;
}

int main(void)
{
    int cons = open("/dev/console", O_WRONLY);
    if (cons >= 0) {
        dup2(cons, 1);
        dup2(cons, 2);
        if (cons > 2)
            close(cons);
    }
    setvbuf(stdout, NULL, _IONBF, 0);

    setenv("PATH", "/bin:/sbin:" LTP_BIN, 1);
    setenv("TMPDIR", "/tmp", 1);
    setenv("LTP_IPC_PATH", "/tmp/ltp-ipc", 1);
    setenv("HOME", "/", 1);

    chdir("/tmp");

    FILE *f = fopen(CHUNK_FILE, "r");
    if (!f) {
        printf("SCAN_BEGIN\nchunk-open-failed: %s\nSCAN_DONE\n", CHUNK_FILE);
        fflush(stdout);
    } else {
        char line[256];
        printf("SCAN_BEGIN\n");
        fflush(stdout);
        while (fgets(line, sizeof(line), f)) {
            size_t n = strlen(line);
            while (n > 0 && (line[n - 1] == '\n' || line[n - 1] == '\r'))
                line[--n] = '\0';
            if (n == 0 || line[0] == '#')
                continue;
            run_one(line);
        }
        fclose(f);
        printf("SCAN_DONE\n");
        fflush(stdout);
    }

    /* PID 1 parks forever: init exit would take the kernel down */
    for (;;)
        sleep(3600);
    return 0;
}
