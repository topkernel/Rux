/*
 * Test: fsync() / fdatasync()
 * Test file synchronization
 */

#include <unistd.h>
#include <fcntl.h>
#include <string.h>

int main(void)
{
    int fd;
    const char *msg = "sync test";

    /* Create test file */
    fd = open("/tmp/test_fsync.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;

    /* Write data */
    if (write(fd, msg, strlen(msg)) < 0) {
        close(fd);
        unlink("/tmp/test_fsync.txt");
        return 2;
    }

    /* Sync to disk */
    if (fsync(fd) < 0) {
        close(fd);
        unlink("/tmp/test_fsync.txt");
        return 3;
    }

    close(fd);
    unlink("/tmp/test_fsync.txt");

    return 0;
}
