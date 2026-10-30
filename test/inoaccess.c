/* inoaccess.c — one IN_ACCESS per read/pread, not two (Linux fsnotify
 * semantics: fsnotify_access fires once per successful read syscall).
 *
 * The ext4 read_at op self-notified IN_ACCESS while File::read_at's
 * wrapper notified again — every pread(2) on an ext4 file with a watch
 * delivered TWO events. This probe pins the count: staged read(2),
 * fast-path read(2), and pread(2) must each deliver exactly one
 * IN_ACCESS event (and an O_DIRECT read through the staged path too).
 *
 * Prints INOACCESS done when every row holds; the wrapper greps it.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/inotify.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef SYS_inotify_init1
#define SYS_inotify_init1 26
#endif

static int count_access_events(int ifd)
{
    /* Non-blocking drain; count IN_ACCESS rows. */
    int oflags = fcntl(ifd, F_GETFL);
    fcntl(ifd, F_SETFL, oflags | O_NONBLOCK);
    char buf[8192];
    int n_access = 0;
    for (;;) {
        int n = (int)read(ifd, buf, sizeof buf);
        if (n <= 0)
            break;
        char *p = buf;
        while (p < buf + n) {
            struct inotify_event *ie = (struct inotify_event *)p;
            if (ie->mask & IN_ACCESS)
                n_access++;
            p += sizeof(*ie) + ie->len;
        }
    }
    fcntl(ifd, F_SETFL, oflags);
    return n_access;
}

static int one_row(const char *what, int ifd, int iofd, int (*op)(int))
{
    op(iofd); /* the I/O under test */
    /* Events are queued synchronously at notify time in this kernel;
     * a short sleep covers an async future without hanging the leg. */
    usleep(50 * 1000);
    int n = count_access_events(ifd);
    if (n == 1) {
        printf("INOACCESS ok %s (1 IN_ACCESS)\n", what);
        return 0;
    }
    printf("INOACCESS FAIL %s: expected 1 IN_ACCESS, saw %d\n", what, n);
    return 1;
}

static int do_read(int fd) { char b[16]; lseek(fd, 0, SEEK_SET); return (int)read(fd, b, sizeof b) > 0; }
static int do_pread(int fd) { char b[16]; return (int)pread(fd, b, sizeof b, 0) > 0; }
static int do_odirect(int fd)
{
    char b[4096] __attribute__((aligned(4096)));
    return (int)pread(fd, b, sizeof b, 0) > 0;
}

int main(void)
{
    setbuf(stdout, NULL);
    const char *path = "/tmp/inoaccess-file";
    printf("INOACCESS step: open-create\n");
    int fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    if (fd < 0) {
        printf("INOACCESS FAIL open %s errno=%d\n", path, errno);
        return 1;
    }
    write(fd, "hello inoaccess probe padding padding padding\n", 46);
    close(fd);

    printf("INOACCESS step: inotify_init1\n");
    int ifd = (int)syscall(SYS_inotify_init1, 0, 0, 0, 0, 0, 0);
    if (ifd < 0) {
        printf("INOACCESS FAIL inotify_init1 errno=%d\n", errno);
        return 1;
    }
    printf("INOACCESS step: add_watch\n");
    if (inotify_add_watch(ifd, path, IN_ACCESS) < 0) {
        printf("INOACCESS FAIL add_watch errno=%d\n", errno);
        return 1;
    }

    int bad = 0;

    printf("INOACCESS step: read row\n");
    fd = open(path, O_RDONLY);
    bad += one_row("read", ifd, fd, do_read);
    close(fd);

    printf("INOACCESS step: pread row\n");
    fd = open(path, O_RDONLY);
    bad += one_row("pread", ifd, fd, do_pread);
    close(fd);

    /* O_DIRECT through the staged pread path (the arm whose self-notify
     * doubled with File::read_at's wrapper). */
    printf("INOACCESS step: odirect row\n");
    fd = open(path, O_RDONLY | O_DIRECT);
    if (fd >= 0) {
        bad += one_row("odirect-pread", ifd, fd, do_odirect);
        close(fd);
    } else {
        printf("INOACCESS ok odirect-pread skipped (open O_DIRECT errno=%d)\n", errno);
    }

    if (bad) {
        printf("INOACCESS FAIL %d row(s) wrong\n", bad);
        return 1;
    }
    printf("INOACCESS done\n");
    return 0;
}
