/*
 * Test: open/read/write/close
 * 测试文件 I/O 操作
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

    /* 创建测试文件 */
    fd = open("/tmp/test_fileio.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;

    /* 写入数据 */
    len = write(fd, test_str, strlen(test_str));
    if (len != (ssize_t)strlen(test_str)) {
        close(fd);
        return 2;
    }
    close(fd);

    /* 读取并验证 */
    fd = open("/tmp/test_fileio.txt", O_RDONLY);
    if (fd < 0) return 3;

    len = read(fd, buf, sizeof(buf) - 1);
    if (len < 0) {
        close(fd);
        return 4;
    }
    buf[len] = '\0';
    close(fd);

    /* 验证内容 */
    if (strcmp(buf, test_str) != 0) return 5;

    /* 清理 */
    unlink("/tmp/test_fileio.txt");

    return 0;
}
