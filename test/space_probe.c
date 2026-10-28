/* space_probe.c — measure ext4 free-space/inode reclaim on unlink.
 * Run as /probe_space on the LTP rootfs. Reports statfs before/after each
 * phase so we can see whether unlink returns blocks+inodes.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/types.h>
#include <unistd.h>

static void report(const char *tag)
{
    struct statvfs st;
    if (statvfs("/tmp", &st) < 0) {
        printf("%s: statvfs failed: %s\n", tag, strerror(errno));
        return;
    }
    printf("%s: blocks=%llu free=%llu avail=%llu files=%llu ffree=%llu\n",
           tag, (unsigned long long)st.f_blocks, (unsigned long long)st.f_bfree,
           (unsigned long long)st.f_bavail, (unsigned long long)st.f_files,
           (unsigned long long)st.f_ffree);
    fflush(stdout);
}

static int write_file(const char *path, const char *data, ssize_t len)
{
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0)
        return -1;
    if (len > 0 && write(fd, data, len) != len) {
        close(fd);
        return -1;
    }
    close(fd);
    return 0;
}

int main(void)
{
    char path[256];
    int i, r, round, fail = 0;

    printf("PROBE-SPACE-START\n");
    report("boot");

    /* phase 1: 400 small files, then unlink */
    for (i = 0; i < 400; i++) {
        snprintf(path, sizeof(path), "/tmp/s_%d", i);
        if (write_file(path, "file\n", 5) < 0) {
            printf("phase1: create %s failed: %s\n", path, strerror(errno));
            fail = 1;
            break;
        }
    }
    report("after 400 small files");
    for (i = 0; i < 400; i++) {
        snprintf(path, sizeof(path), "/tmp/s_%d", i);
        unlink(path);
    }
    sync();
    report("after unlink small files");

    /* phase 2: 60MB file, then unlink */
    {
        int fd = open("/tmp/big", O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (fd < 0) {
            printf("phase2: open /tmp/big failed: %s\n", strerror(errno));
            fail = 1;
        } else {
            char buf[4096];
            memset(buf, 0x41, sizeof(buf));
            ssize_t total = 0;
            while (total < 60LL * 1024 * 1024) {
                ssize_t n = write(fd, buf, sizeof(buf));
                if (n < 0) {
                    printf("phase2: write failed at %lld: %s\n",
                           (long long)total, strerror(errno));
                    fail = 1;
                    break;
                }
                total += n;
            }
            close(fd);
            sync();
            report("after 60MB file");
            unlink("/tmp/big");
            sync();
            report("after unlink 60MB");
        }
    }

    /* phase 3: churn 30 rounds x 20 files create+delete */
    for (round = 0; round < 30; round++) {
        for (i = 0; i < 20; i++) {
            snprintf(path, sizeof(path), "/tmp/c_%d_%d", round, i);
            if (write_file(path, "churn\n", 6) < 0) {
                printf("phase3: round %d create failed: %s\n", round,
                       strerror(errno));
                fail = 1;
                round = 100;
                break;
            }
        }
        for (i = 0; i < 20; i++) {
            snprintf(path, sizeof(path), "/tmp/c_%d_%d", round, i);
            unlink(path);
        }
    }
    sync();
    report("after churn 30x20");

    /* phase 4: 5000 files then unlink — inode pressure */
    for (i = 0; i < 5000; i++) {
        snprintf(path, sizeof(path), "/tmp/i_%d", i);
        if (write_file(path, "x", 1) < 0) {
            printf("phase4: create %d failed: %s\n", i, strerror(errno));
            fail = 1;
            break;
        }
    }
    report("after 5000 files");
    for (r = 0; r < i; r++) {
        snprintf(path, sizeof(path), "/tmp/i_%d", r);
        unlink(path);
    }
    sync();
    report("after unlink 5000");

    printf("PROBE-SPACE-DONE fail=%d\n", fail);
    fflush(stdout);
    return 0;
}
