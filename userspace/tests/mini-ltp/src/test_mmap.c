/*
 * Test: mmap() / munmap()
 * 测试内存映射
 */

#include <sys/mman.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>

int main(void)
{
    void *addr;
    size_t size = 4096;
    const char *msg = "mmap test data";

    /* 测试匿名映射 */
    addr = mmap(NULL, size, PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);

    if (addr == (void *)-1) return 1;

    /* 写入数据 */
    strcpy((char *)addr, msg);

    /* 读取验证 */
    if (strcmp((char *)addr, msg) != 0) {
        munmap(addr, size);
        return 2;
    }

    /* 取消映射 */
    if (munmap(addr, size) < 0) return 3;

    return 0;
}
