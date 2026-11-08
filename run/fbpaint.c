/* fbpaint — write a test pattern straight into /dev/fb0.
 *
 * Isolation probe for the "black host window" question: if a direct
 * write into the fbdev mapping shows up in the host's screendump, the
 * kernel's display chain (mmap -> backing -> TRANSFER_TO_HOST_2D ->
 * RESOURCE_FLUSH -> scanout) is intact and the blank screen belongs
 * to the X/mutter side; if it does NOT show, the kernel's display
 * path is the broken link.
 *
 * Exit codes: 0 = painted (and the pattern was written), 1 = open
 * failed, 2 = mmap failed, 3 = write failed.
 */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

struct fb_fix {
    unsigned long id;
    unsigned long smem_start;
    unsigned long smem_len;
    unsigned long type;
    unsigned long type_aux;
    unsigned long visual;
    unsigned short xpanstep, ypanstep, ywrapstep;
    unsigned long mmio_start;
    unsigned long mmio_len;
    unsigned int accel;
    unsigned short capabilities;
    unsigned short reserved[2];
};

int main(void)
{
    int fd = open("/dev/fb0", O_RDWR);
    if (fd < 0) {
        printf("FBPAINT: open /dev/fb0 failed\n");
        return 1;
    }
    struct fb_fix fix;
    memset(&fix, 0, sizeof fix);
    if (ioctl(fd, 0x4602 /* FBIOGET_FSCREENINFO */, &fix) == 0) {
        printf("FBPAINT: smem_start=%#lx smem_len=%#lx type=%#lx\n",
               fix.smem_start, fix.smem_len, fix.type);
    } else {
        printf("FBPAINT: FBIOGET_FSCREENINFO failed\n");
    }
    void *p = mmap(NULL, 0x3e8000, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (p == MAP_FAILED) {
        printf("FBPAINT: mmap failed\n");
        return 2;
    }
    /* 1280x800x4 pattern: red/green stripes, then a white block. */
    unsigned char *fb = (unsigned char *)p;
    for (int y = 0; y < 800; y++) {
        for (int x = 0; x < 1280; x++) {
            unsigned char *px = fb + (size_t)y * 5120 + (size_t)x * 4;
            if (y < 400) {
                px[0] = 0xff; px[1] = 0x00; px[2] = 0x00; px[3] = 0xff; /* red */
            } else if (x < 640) {
                px[0] = 0x00; px[1] = 0xff; px[2] = 0x00; px[3] = 0xff; /* green */
            } else {
                px[0] = 0xff; px[1] = 0xff; px[2] = 0xff; px[3] = 0xff; /* white */
            }
        }
    }
    printf("FBPAINT: pattern written\n");
    fflush(stdout);
    sleep(30);
    return 0;
}
