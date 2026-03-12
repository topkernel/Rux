/*
 * Test: mmap() / munmap()
 * Test memory mapping
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

    /* Test anonymous mapping */
    addr = mmap(NULL, size, PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);

    if (addr == (void *)-1) return 1;

    /* Write data */
    strcpy((char *)addr, msg);

    /* Read and verify */
    if (strcmp((char *)addr, msg) != 0) {
        munmap(addr, size);
        return 2;
    }

    /* Unmap */
    if (munmap(addr, size) < 0) return 3;

    return 0;
}
