/*
 * Test: open/read/write/close
 * Test file I/O operations
 */

#include <fcntl.h>
#include <unistd.h>
#include <string.h>

int main(void)
{
    int fd;
    char buf[64];
    const char *test_str = "Hello Rux OS!";
    ssize_t len;

    /* Create test file */
    fd = open("/tmp/test_fileio.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;

    /* Write data */
    len = write(fd, test_str, strlen(test_str));
    if (len != (ssize_t)strlen(test_str)) {
        close(fd);
        return 2;
    }
    close(fd);

    /* Read and verify */
    fd = open("/tmp/test_fileio.txt", O_RDONLY);
    if (fd < 0) return 3;

    len = read(fd, buf, sizeof(buf) - 1);
    if (len < 0) {
        close(fd);
        return 4;
    }
    buf[len] = '\0';
    close(fd);

    /* Verify content */
    if (strcmp(buf, test_str) != 0) return 5;

    /* Cleanup */
    unlink("/tmp/test_fileio.txt");

    return 0;
}
