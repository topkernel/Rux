/*
 * Test: lseek()
 * Test file positioning
 */

#include <unistd.h>
#include <fcntl.h>
#include <string.h>

int main(void)
{
    const char *path = "/tmp/test_lseek.txt";
    const char *data = "0123456789ABCDEF";
    int fd;
    char buf[8];
    off_t pos;

    /* Create test file */
    fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;
    write(fd, data, strlen(data));
    close(fd);

    /* Open and test lseek */
    fd = open(path, O_RDONLY);
    if (fd < 0) {
        unlink(path);
        return 2;
    }

    /* SEEK_SET */
    pos = lseek(fd, 5, SEEK_SET);
    if (pos != 5) {
        close(fd);
        unlink(path);
        return 3;
    }

    if (read(fd, buf, 3) != 3 || strncmp(buf, "567", 3) != 0) {
        close(fd);
        unlink(path);
        return 4;
    }

    /* SEEK_CUR */
    pos = lseek(fd, 2, SEEK_CUR);
    if (pos != 10) {
        close(fd);
        unlink(path);
        return 5;
    }

    if (read(fd, buf, 3) != 3 || strncmp(buf, "CDE", 3) != 0) {
        close(fd);
        unlink(path);
        return 6;
    }

    /* SEEK_END */
    pos = lseek(fd, -4, SEEK_END);
    if (pos != 12) {
        close(fd);
        unlink(path);
        return 7;
    }

    if (read(fd, buf, 4) != 4 || strncmp(buf, "CDEF", 4) != 0) {
        close(fd);
        unlink(path);
        return 8;
    }

    close(fd);
    unlink(path);

    return 0;
}
