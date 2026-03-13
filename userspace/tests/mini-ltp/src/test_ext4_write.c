/*
 * Test: ext4 write support
 * Test writing to a pre-existing file on ext4 filesystem
 */

#include <fcntl.h>
#include <unistd.h>
#include <string.h>
#include <stdio.h>

int main(void)
{
    int fd;
    const char *test_file = "/test/writetest.txt";
    const char *write_data = "EXT4_WRITE_TEST_SUCCESS";
    ssize_t len;
    char buf[64];

    /* Open existing file for writing (no O_CREAT) */
    fd = open(test_file, O_WRONLY | O_TRUNC);
    if (fd < 0) {
        printf("ext4_write: open failed (fd=%d)\n", fd);
        return 1;
    }

    /* Write data */
    len = write(fd, write_data, strlen(write_data));
    if (len != (ssize_t)strlen(write_data)) {
        printf("ext4_write: write failed (len=%zd, expected=%zu)\n", len, strlen(write_data));
        close(fd);
        return 2;
    }
    close(fd);

    /* Read back and verify */
    fd = open(test_file, O_RDONLY);
    if (fd < 0) {
        printf("ext4_write: reopen failed\n");
        return 3;
    }

    len = read(fd, buf, sizeof(buf) - 1);
    if (len < 0) {
        printf("ext4_write: read failed\n");
        close(fd);
        return 4;
    }
    buf[len] = '\0';
    close(fd);

    /* Verify content */
    if (strcmp(buf, write_data) != 0) {
        printf("ext4_write: content mismatch (got='%s', expected='%s')\n", buf, write_data);
        return 5;
    }

    printf("ext4_write: PASS\n");
    return 0;
}
