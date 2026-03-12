/*
 * Test: stat() / fstat()
 * Test file status retrieval
 */

#include <sys/stat.h>
#include <fcntl.h>
#include <unistd.h>
#include <string.h>

int main(void)
{
    struct stat st;
    int fd;
    const char *path = "/tmp/test_stat.txt";
    const char *msg = "stat test content";

    /* Create test file */
    fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;
    write(fd, msg, strlen(msg));
    close(fd);

    /* Test stat() */
    if (stat(path, &st) < 0) {
        unlink(path);
        return 2;
    }

    /* Verify file size */
    if (st.st_size != (off_t)strlen(msg)) {
        unlink(path);
        return 3;
    }

    /* Verify file type */
    if (!S_ISREG(st.st_mode)) {
        unlink(path);
        return 4;
    }

    /* Test fstat() */
    fd = open(path, O_RDONLY);
    if (fd < 0) {
        unlink(path);
        return 5;
    }

    if (fstat(fd, &st) < 0) {
        close(fd);
        unlink(path);
        return 6;
    }

    close(fd);
    unlink(path);

    return 0;
}
