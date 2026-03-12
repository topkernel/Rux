/*
 * Test: access()
 * Test file access permission check
 */

#include <unistd.h>
#include <fcntl.h>

int main(void)
{
    const char *path = "/tmp/test_access.txt";
    int fd;

    /* Create test file */
    fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;
    close(fd);

    /* Test F_OK */
    if (access(path, F_OK) < 0) {
        unlink(path);
        return 2;
    }

    /* Test R_OK */
    if (access(path, R_OK) < 0) {
        unlink(path);
        return 3;
    }

    /* Test W_OK */
    if (access(path, W_OK) < 0) {
        unlink(path);
        return 4;
    }

    /* Test non-existent file */
    if (access("/tmp/nonexistent_file_xyz", F_OK) == 0) {
        unlink(path);
        return 5;
    }

    unlink(path);
    return 0;
}
