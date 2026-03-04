/*
 * Test: fsync() / fdatasync()
 * 测试文件同步
 */

#include <unistd.h>
#include <fcntl.h>
#include <string.h>

int main(void)
{
    int fd;
    const char *msg = "sync test";

    /* 创建测试文件 */
    fd = open("/tmp/test_fsync.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;

    /* 写入数据 */
    if (write(fd, msg, strlen(msg)) < 0) {
        close(fd);
        unlink("/tmp/test_fsync.txt");
        return 2;
    }

    /* 同步到磁盘 */
    if (fsync(fd) < 0) {
        close(fd);
        unlink("/tmp/test_fsync.txt");
        return 3;
    }

    close(fd);
    unlink("/tmp/test_fsync.txt");

    return 0;
}
