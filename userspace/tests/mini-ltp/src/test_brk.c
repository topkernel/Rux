/*
 * Test: brk() / sbrk()
 * Test heap memory management
 */

#include <unistd.h>
#include <string.h>

int main(void)
{
    void *orig_brk;
    void *new_brk;
    char *ptr;

    /* Get current heap position */
    orig_brk = sbrk(0);
    if (orig_brk == (void *)-1) return 1;

    /* Expand heap by 4096 bytes */
    if (brk((char *)orig_brk + 4096) < 0) return 2;

    new_brk = sbrk(0);
    if (new_brk == (void *)-1) return 3;

    /* Verify heap has expanded */
    if ((char *)new_brk - (char *)orig_brk < 4096) return 4;

    /* Use newly allocated memory */
    ptr = (char *)orig_brk;
    strcpy(ptr, "brk test");
    if (strcmp(ptr, "brk test") != 0) return 5;

    /* Restore heap */
    if (brk(orig_brk) < 0) return 6;

    return 0;
}
