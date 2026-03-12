/*
 * Test: rename()
 * Test file renaming
 */

#include <unistd.h>
#include <fcntl.h>
#include <string.h>

int main(void)
{
    const char *old_path = "/tmp/test_rename_old.txt";
    const char *new_path = "/tmp/test_rename_new.txt";
    const char *msg = "rename test";
    int fd;
    char buf[64];
    ssize_t len;

    /* Create test file */
    fd = open(old_path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;
    write(fd, msg, strlen(msg));
    close(fd);

    /* Rename */
    if (rename(old_path, new_path) < 0) {
        unlink(old_path);
        return 2;
    }

    /* Verify old file does not exist */
    if (access(old_path, F_OK) == 0) {
        unlink(old_path);
        unlink(new_path);
        return 3;
    }

    /* Verify new file exists and has correct content */
    fd = open(new_path, O_RDONLY);
    if (fd < 0) {
        unlink(new_path);
        return 4;
    }

    len = read(fd, buf, sizeof(buf) - 1);
    close(fd);

    if (len != (ssize_t)strlen(msg) || strncmp(buf, msg, len) != 0) {
        unlink(new_path);
        return 5;
    }

    unlink(new_path);
    return 0;
}
