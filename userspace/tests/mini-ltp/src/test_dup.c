/*
 * Test: dup() / dup2()
 * 测试文件描述符复制
 */

#include <unistd.h>
#include <fcntl.h>
#include <string.h>

int main(void)
{
    int fd1, fd2, fd3;
    char buf[32];
    const char *msg = "dup test";

    /* 创建测试文件 */
    fd1 = open("/tmp/test_dup.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd1 < 0) return 1;

    /* 测试 dup() */
    fd2 = dup(fd1);
    if (fd2 < 0) {
        close(fd1);
        return 2;
    }

    /* 通过 dup 的 fd 写入 */
    if (write(fd2, msg, strlen(msg)) != (ssize_t)strlen(msg)) {
        close(fd1);
        close(fd2);
        return 3;
    }

    /* 测试 dup2() */
    fd3 = dup2(fd1, 100);
    if (fd3 != 100) {
        close(fd1);
        close(fd2);
        return 4;
    }

    close(fd1);
    close(fd2);
    close(fd3);

    /* 验证写入内容 */
    fd1 = open("/tmp/test_dup.txt", O_RDONLY);
    if (fd1 < 0) return 5;

    ssize_t len = read(fd1, buf, sizeof(buf) - 1);
    close(fd1);

    if (len != (ssize_t)strlen(msg)) return 6;

    unlink("/tmp/test_dup.txt");
    return 0;
}
