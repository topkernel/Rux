/*
 * Test: writev() / readv()
 * 测试向量 I/O
 */

#include <sys/uio.h>
#include <fcntl.h>
#include <unistd.h>
#include <string.h>

int main(void)
{
    const char *path = "/tmp/test_writev.txt";
    int fd;
    struct iovec iov[3];
    char buf1[16], buf2[16], buf3[16];
    char rbuf1[16], rbuf2[16], rbuf3[16];
    struct iovec riov[3];
    ssize_t n;

    /* 准备数据 */
    strcpy(buf1, "Hello");
    strcpy(buf2, " ");
    strcpy(buf3, "World");

    iov[0].iov_base = buf1;
    iov[0].iov_len = strlen(buf1);
    iov[1].iov_base = buf2;
    iov[1].iov_len = strlen(buf2);
    iov[2].iov_base = buf3;
    iov[2].iov_len = strlen(buf3);

    /* 写入 */
    fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;

    n = writev(fd, iov, 3);
    close(fd);

    if (n != (ssize_t)(strlen(buf1) + strlen(buf2) + strlen(buf3))) return 2;

    /* 读取验证 */
    fd = open(path, O_RDONLY);
    if (fd < 0) {
        unlink(path);
        return 3;
    }

    riov[0].iov_base = rbuf1;
    riov[0].iov_len = sizeof(rbuf1);
    riov[1].iov_base = rbuf2;
    riov[1].iov_len = sizeof(rbuf2);
    riov[2].iov_base = rbuf3;
    riov[2].iov_len = sizeof(rbuf3);

    n = readv(fd, riov, 3);
    close(fd);
    unlink(path);

    if (n < (ssize_t)(strlen(buf1) + strlen(buf2) + strlen(buf3))) return 4;

    /* 验证内容 */
    if (strncmp(rbuf1, "Hello", 5) != 0) return 5;

    return 0;
}
