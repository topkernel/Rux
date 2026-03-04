/*
 * Test: mkdir() / rmdir()
 * 测试目录操作
 */

#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#include <dirent.h>

int main(void)
{
    const char *dir_path = "/tmp/test_mini_ltp_dir";
    DIR *dir;

    /* 创建目录 */
    if (mkdir(dir_path, 0755) < 0) return 1;

    /* 验证目录存在 */
    dir = opendir(dir_path);
    if (dir == NULL) {
        rmdir(dir_path);
        return 2;
    }

    /* 关闭目录 */
    if (closedir(dir) < 0) {
        rmdir(dir_path);
        return 3;
    }

    /* 删除目录 */
    if (rmdir(dir_path) < 0) return 4;

    /* 验证目录已删除 */
    dir = opendir(dir_path);
    if (dir != NULL) {
        closedir(dir);
        return 5;
    }

    return 0;
}
