/*
 * Test: chdir() / getcwd()
 * 测试目录切换
 */

#include <unistd.h>
#include <string.h>

int main(void)
{
    char cwd[256];
    char new_cwd[256];

    /* 获取当前目录 */
    if (getcwd(cwd, sizeof(cwd)) == NULL) return 1;

    /* 切换到 /tmp */
    if (chdir("/tmp") < 0) return 2;

    /* 验证目录已切换 */
    if (getcwd(new_cwd, sizeof(new_cwd)) == NULL) return 3;

    if (strcmp(new_cwd, "/tmp") != 0) return 4;

    /* 切换回原目录 */
    if (chdir(cwd) < 0) return 5;

    return 0;
}
