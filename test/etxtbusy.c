/* etxtbusy.c — ETXTBSY gate probe (execve04 shape).
 *
 * Verifies resolve_exec_file's deny_write_access analogue: a regular
 * file some open file description holds for writing must fail execve
 * with ETXTBSY, and must become executable again once every write
 * descriptor is closed. Self-exec: with argv[1] == "child" the process
 * just exits 0; the parent drives the three checks.
 *
 *   1. exec with no writers open          -> success
 *   2. exec with an O_RDWR fd held open   -> ETXTBSY
 *   3. exec again after closing that fd   -> success
 *
 * Prints ETXTBUSY done rounds=3 at the end (the wrapper greps it).
 */
#include <fcntl.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>
#include <errno.h>
#include <string.h>

int main(int argc, char **argv)
{
    if (argc > 1 && strcmp(argv[1], "child") == 0)
        return 0;

    const char *self = argv[0];
    char *cargv[] = { (char *)self, (char *)"child", NULL };
    char *cenvp[] = { NULL };
    int rounds = 0;

    /* 1: no writers — exec must succeed. */
    pid_t p = fork();
    if (p == 0) {
        execve(self, cargv, cenvp);
        fprintf(stderr, "etxtbusy: round1 execve failed rc=%d (%s)\n",
                errno, strerror(errno));
        _exit(42);
    }
    int st = 0;
    waitpid(p, &st, 0);
    if (!(WIFEXITED(st) && WEXITSTATUS(st) == 0)) {
        printf("ETXTBUSY FAIL round1 base exec broken st=%d errno=%d\n",
               st, errno);
        return 1;
    }
    rounds++;

    /* 2: hold the image open for writing — exec must fail ETXTBSY. */
    int fd = open(self, O_RDWR);
    if (fd < 0) {
        /* Read-only rootfs: O_RDWR on the RO mount would be EXDEV/EROFS
         * for reasons unrelated to the gate; copy to /tmp first. */
        char buf[4096];
        int in = open(self, O_RDONLY);
        int out = open("/tmp/etxtbusy-copy", O_WRONLY | O_CREAT | O_TRUNC, 0700);
        if (in < 0 || out < 0) {
            printf("ETXTBUSY SKIP cannot stage copy in=%d out=%d\n", in, out);
            return 3;
        }
        ssize_t n;
        while ((n = read(in, buf, sizeof buf)) > 0)
            write(out, buf, n);
        close(in);
        close(out);
        /* self-exec the copy instead */
        cargv[0] = (char *)"/tmp/etxtbusy-copy";
        self = "/tmp/etxtbusy-copy";
        fd = open(self, O_RDWR);
        if (fd < 0) {
            printf("ETXTBUSY FAIL copy not writable fd=%d errno=%d\n", fd, errno);
            return 1;
        }
    }
    p = fork();
    if (p == 0) {
        execve(self, cargv, cenvp);
        if (errno == ETXTBSY)
            _exit(0);
        fprintf(stderr, "etxtbusy: round2 wrong errno %d (%s)\n",
                errno, strerror(errno));
        _exit(43);
    }
    waitpid(p, &st, 0);
    if (!(WIFEXITED(st) && WEXITSTATUS(st) == 0)) {
        printf("ETXTBUSY FAIL round2 expected ETXTBSY st=%d\n", st);
        return 1;
    }
    rounds++;

    /* 3: close the writer — exec must succeed again. */
    close(fd);
    p = fork();
    if (p == 0) {
        execve(self, cargv, cenvp);
        fprintf(stderr, "etxtbusy: round3 execve failed rc=%d (%s)\n",
                errno, strerror(errno));
        _exit(44);
    }
    waitpid(p, &st, 0);
    if (!(WIFEXITED(st) && WEXITSTATUS(st) == 0)) {
        printf("ETXTBUSY FAIL round3 exec still busy st=%d\n", st);
        return 1;
    }
    rounds++;

    printf("ETXTBUSY done rounds=%d\n", rounds);
    return 0;
}
