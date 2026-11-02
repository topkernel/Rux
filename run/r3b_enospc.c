// r3b ENOSPC partial-write probe (review-r3b lane A/B for the
// ext4_file_write mid-loop partial-count fix).
//
// Shape: a sparse file with a HOLE INSIDE its size (block 0 and block 2
// written, block 1 never touched) on a completely full filesystem. A
// 3-block pwrite at offset 0 then walks block 0 (exists, lands), block 1
// (hole -> allocation fails mid-loop). POSIX write(2)/reference ext4:
// the syscall must report the PARTIAL count (4096), not -ENOSPC, and the
// file must keep its size and prior data.
#define _GNU_SOURCE
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>
#include <string.h>
#include <stdio.h>

static char buf[65536];

int main(void) {
    int fd, rc;
    setvbuf(stdout, NULL, _IONBF, 0);
    /* init has no fd 0/1/2 — attach the console or every printf dies. */
    int cfd = open("/dev/console", O_WRONLY);
    if (cfd >= 0) {
        if (cfd != 1) { dup2(cfd, 1); }
        if (cfd > 2) { close(cfd); }
    }

    // 1. sparse file: blocks 0 and 2 written, block 1 left a hole.
    printf("EP[start]\n");
    unlink("/sparse");
    fd = open("/sparse", O_CREAT | O_WRONLY | O_TRUNC, 0600);
    if (fd < 0) { printf("EP[open sparse errno=%d]\nEP-DONE\n", errno); return 1; }
    memset(buf, 0xAB, 4096);
    if (pwrite(fd, buf, 4096, 0) != 4096) {
        printf("EP[pwrite b0 errno=%d]\nEP-DONE\n", errno);
        close(fd);
        return 1;
    }
    memset(buf + 4096, 0xCD, 4096);
    if (pwrite(fd, buf + 4096, 4096, 2 * 4096) != 4096) {
        printf("EP[pwrite b2 errno=%d]\nEP-DONE\n", errno);
        close(fd);
        return 1;
    }
    close(fd);

    printf("EP[sparse-ok]\n");
    memset(buf, 0x5A, sizeof(buf));
    for (long fno = 0;; fno++) {
        char name[32];
        snprintf(name, sizeof(name), "/fill%04ld", fno);
        int f = open(name, O_CREAT | O_WRONLY | O_TRUNC, 0600);
        if (f < 0) break;
        int hit = 0;
        for (;;) {
            ssize_t w = write(f, buf, sizeof(buf));
            if (w == (ssize_t)sizeof(buf)) continue;
            hit = 1; /* partial or -1 */
            break;
        }
        close(f);
        if (hit) break;
    }
    for (long k = 0; k < 8192; k++) {
        char name[32];
        snprintf(name, sizeof(name), "/t%04ld", k);
        int f = open(name, O_CREAT | O_WRONLY, 0600);
        if (f < 0) break;
        ssize_t w = write(f, buf, 512);
        close(f);
        if (w != 512) break; /* full: stop churning creates */
    }

    printf("EP[full]\n");
    fd = open("/sparse", O_WRONLY);
    if (fd < 0) { printf("EP[reopen errno=%d]\nEP-DONE\n", errno); return 1; }
    memset(buf, 0x11, 3 * 4096);
    errno = 0;
    rc = (int)pwrite(fd, buf, 3 * 4096, 0);
    int e = errno;
    printf("EP[pwrite3 rc=%d errno=%d]\n", rc, e);

    // 4. integrity: block 0 holds the new pattern, block 2 the old one,
    //    size unchanged (12288).
    int ok = 0;
    char back[12288];
    int rfd = open("/sparse", O_RDONLY);
    if (rfd >= 0) {
        int r = (int)pread(rfd, back, sizeof(back), 0);
        int b0 = r == 12288 && back[0] == 0x11 && back[4095] == 0x11;
        int b1hole = back[4096] == 0; /* failed hole alloc reads as zero */
        int b2 = back[2 * 4096] == 0xCD && back[12287] == 0xCD;
        ok = b0 && b2;
        printf("EP[readback n=%d b0=%d b1zero=%d b2=%d]\n", r, b0, b1hole, b2);
        close(rfd);
    }

    // Verdict: partial count of exactly one block, prior data intact.
    int pass = (rc == 4096) && ok;
    printf("EP-RESULT: %s rc=%d errno=%d data=%d\n",
           pass ? "PASS" : "FAIL", rc, e, ok);
    printf("EP-DONE\n");
    return 0;
}
