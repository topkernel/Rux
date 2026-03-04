/*
 * Test: access()
 * 测试文件访问权限检查
 */

#include <unistd.h>
#include <fcntl.h>

int main(void)
{
    const char *path = "/tmp/test_access.txt";
    int fd;

    /* 创建测试文件 */
    fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;
    close(fd);

    /* 测试 F_OK */
    if (access(path, F_OK) < 0) {
        unlink(path);
        return 2;
    }

    /* 测试 R_OK */
    if (access(path, R_OK) < 0) {
        unlink(path);
        return 3;
    }

    /* 测试 W_OK */
    if (access(path, W_OK) < 0) {
        unlink(path);
        return 4;
    }

    /* 测试不存在的文件 */
    if (access("/tmp/nonexistent_file_xyz", F_OK) == 0) {
        unlink(path);
        return 5;
    }

    unlink(path);
    return 0;
}
