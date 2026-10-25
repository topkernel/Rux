// swprobe.c — pinpoint the EISDIR seen by Xorg AIGLX when dlopening the
// pinned mesa 20.0.4 swrast driver.
//
// Booted as init=/swprobe on a private copy of the GNOME image: stat,
// open, pread64, read and dlopen the exact path AIGLX uses, then repeat
// the open+pread cycle under an artificial descriptor/page-cache load
// (the shell test showed a quiet system reads the file fine; Xorg's
// dlopen fails mid-session, so state is a variable).
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static const char *P = "/usr/lib/riscv64-linux-gnu/dri/swrast_dri.so";

static void con_attach(void) {
    int c = open("/dev/console", O_WRONLY);
    if (c >= 0) { dup2(c, 1); dup2(c, 2); }
}

int main(void) {
    con_attach();
    printf("[swprobe] start\n");
    fflush(stdout);

    struct stat st;
    int r = stat(P, &st);
    printf("[swprobe] stat rc=%d mode=%o size=%lld nlink=%d\n",
           r, st.st_mode, (long long)st.st_size, st.st_nlink);
    fflush(stdout);

    int fd = open(P, O_RDONLY);
    printf("[swprobe] open fd=%d errno=%d(%s)\n", fd, errno, strerror(errno));
    fflush(stdout);

    if (fd >= 0) {
        char buf[64];
        memset(buf, 0, sizeof buf);
        errno = 0;
        ssize_t n = pread(fd, buf, 64, 0);
        printf("[swprobe] pread64 n=%zd errno=%d(%s) magic=%02x%02x%02x%02x\n",
               n, errno, strerror(errno),
               (unsigned char)buf[0], (unsigned char)buf[1],
               (unsigned char)buf[2], (unsigned char)buf[3]);
        fflush(stdout);
        errno = 0;
        n = read(fd, buf, 16);
        printf("[swprobe] read n=%zd errno=%d(%s)\n", n, errno, strerror(errno));
        fflush(stdout);
        close(fd);
    }

    void *h = dlopen(P, RTLD_LAZY | RTLD_NOW);
    printf("[swprobe] dlopen=%p %s\n", h, h ? "OK" : dlerror());
    fflush(stdout);

    // Descriptor pressure: hold 64 fds on the file, interleave preads.
    int held[64];
    int nheld = 0;
    for (int i = 0; i < 64; i++) {
        held[i] = open(P, O_RDONLY);
        if (held[i] >= 0) nheld++;
    }
    printf("[swprobe] opened %d extra fds\n", nheld);
    fflush(stdout);
    for (int i = 0; i < nheld; i++) {
        char b[8];
        errno = 0;
        ssize_t n = pread(held[i], b, 8, 0);
        if (n != 8) {
            printf("[swprobe] fd %d: pread n=%zd errno=%d(%s)\n",
                   held[i], n, errno, strerror(errno));
            fflush(stdout);
            break;
        }
    }
    for (int i = 0; i < nheld; i++) close(held[i]);

    printf("[swprobe] done\n");
    fflush(stdout);
    for (;;) pause();
}
