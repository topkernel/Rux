/* sparse_probe.c — reproduce the openat02 O_LARGEFILE shape: seek to
 * 4.1 GiB and write 7 bytes. Measures wall time and statfs deltas so the
 * fix (allocate only touched blocks) vs the bug (allocate everything up
 * to offset) is visible without a full sweep.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/time.h>
#include <unistd.h>

static void report(const char *tag)
{
    struct statvfs st;
    if (statvfs("/tmp", &st) < 0) {
        printf("%s: statvfs: %s\n", tag, strerror(errno));
        return;
    }
    printf("%s: free_blocks=%llu\n", tag, (unsigned long long)st.f_bfree);
    fflush(stdout);
}

int main(void)
{
    printf("SPARSE-PROBE-START\n");
    report("before");

    int fd = open("/tmp/sparse_test", O_RDWR | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        printf("open failed: %s\n", strerror(errno));
        return 1;
    }
    off_t target = (off_t)4.1 * 1024 * 1024 * 1024;
    if (lseek(fd, target, SEEK_SET) < 0) {
        printf("lseek failed: %s\n", strerror(errno));
        return 1;
    }
    struct timeval t0, t1;
    gettimeofday(&t0, NULL);
    ssize_t n = write(fd, "abcdefg", 7);
    gettimeofday(&t1, NULL);
    long ms = (t1.tv_sec - t0.tv_sec) * 1000 + (t1.tv_usec - t0.tv_usec) / 1000;
    printf("write at 4.1G: n=%zd errno=%d (%s) took %ld ms\n",
           n, errno, n == 7 ? "ok" : strerror(errno), ms);
    report("after-write");

    struct stat st;
    if (fstat(fd, &st) == 0)
        printf("fstat size=%lld (want %lld)\n", (long long)st.st_size,
               (long long)target + 7);

    /* read back around the write: 16 bytes hole + the data */
    if (lseek(fd, target - 16, SEEK_SET) >= 0) {
        char buf[32];
        memset(buf, 0xAA, sizeof(buf));
        ssize_t r = read(fd, buf, 32);
        printf("read at 4.1G-16: n=%zd\n", r);
        printf("bytes:");
        for (int i = 0; i < (r > 0 ? (int)r : 0); i++)
            printf(" %02x", (unsigned char)buf[i]);
        printf("\n");
    }
    close(fd);
    unlink("/tmp/sparse_test");
    report("after-unlink");
    printf("SPARSE-PROBE-DONE\n");
    return 0;
}
