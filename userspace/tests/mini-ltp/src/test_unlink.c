/*
 * Test: unlink()
 * 测试文件删除
 */

#include <unistd.h>
#include <fcntl.h>

int main(void)
{
    const char *path = "/tmp/test_unlink.txt";
    int fd;

    /* 创建测试文件 */
    fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;
    close(fd);

    /* 验证文件存在 */
    if (access(path, F_OK) < 0) return 2;

    /* 删除文件 */
    if (unlink(path) < 0) return 3;

    /* 验证文件已删除 */
    if (access(path, F_OK) == 0) return 4;

    return 0;
}
