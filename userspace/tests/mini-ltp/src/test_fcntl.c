/*
 * Test: fcntl() - F_GETFD / F_SETFD
 * 测试文件控制
 */

#include <fcntl.h>
#include <unistd.h>

int main(void)
{
    int fd;
    int flags;

    /* 打开文件 */
    fd = open("/tmp/test_fcntl.txt", O_RDWR | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;

    /* 获取标志 */
    flags = fcntl(fd, F_GETFD);
    if (flags < 0) {
        close(fd);
        unlink("/tmp/test_fcntl.txt");
        return 2;
    }

    /* 设置 FD_CLOEXEC */
    if (fcntl(fd, F_SETFD, flags | FD_CLOEXEC) < 0) {
        close(fd);
        unlink("/tmp/test_fcntl.txt");
        return 3;
    }

    /* 验证设置 */
    flags = fcntl(fd, F_GETFD);
    if (!(flags & FD_CLOEXEC)) {
        close(fd);
        unlink("/tmp/test_fcntl.txt");
        return 4;
    }

    close(fd);
    unlink("/tmp/test_fcntl.txt");

    return 0;
}
