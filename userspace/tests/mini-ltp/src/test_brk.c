/*
 * Test: brk() / sbrk()
 * 测试堆内存管理
 */

#include <unistd.h>
#include <string.h>

int main(void)
{
    void *orig_brk;
    void *new_brk;
    char *ptr;

    /* 获取当前堆位置 */
    orig_brk = sbrk(0);
    if (orig_brk == (void *)-1) return 1;

    /* 扩展堆 4096 字节 */
    if (brk((char *)orig_brk + 4096) < 0) return 2;

    new_brk = sbrk(0);
    if (new_brk == (void *)-1) return 3;

    /* 验证堆已扩展 */
    if ((char *)new_brk - (char *)orig_brk < 4096) return 4;

    /* 使用新分配的内存 */
    ptr = (char *)orig_brk;
    strcpy(ptr, "brk test");
    if (strcmp(ptr, "brk test") != 0) return 5;

    /* 恢复堆 */
    if (brk(orig_brk) < 0) return 6;

    return 0;
}
