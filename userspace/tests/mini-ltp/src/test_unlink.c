/*
 * Test: unlink()
 * Test file deletion
 */

#include <unistd.h>
#include <fcntl.h>

int main(void)
{
    const char *path = "/tmp/test_unlink.txt";
    int fd;

    /* Create test file */
    fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;
    close(fd);

    /* Verify file exists */
    if (access(path, F_OK) < 0) return 2;

    /* Delete file */
    if (unlink(path) < 0) return 3;

    /* Verify file has been deleted */
    if (access(path, F_OK) == 0) return 4;

    return 0;
}
