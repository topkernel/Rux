/*
 * Test: stat() / fstat()
 * 测试文件状态获取
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

    /* 创建测试文件 */
    fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;
    write(fd, msg, strlen(msg));
    close(fd);

    /* 测试 stat() */
    if (stat(path, &st) < 0) {
        unlink(path);
        return 2;
    }

    /* 验证文件大小 */
    if (st.st_size != (off_t)strlen(msg)) {
        unlink(path);
        return 3;
    }

    /* 验证文件类型 */
    if (!S_ISREG(st.st_mode)) {
        unlink(path);
        return 4;
    }

    /* 测试 fstat() */
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
