/* glmprobe — repro for the render_service llvmpipe assert (jit_tex->base).
 *
 * Mesa's kms sw winsys flow per frame:
 *   drmPrimeFDToHandle(render_fd, prime_fd) -> handle     (dedupe: same
 *       dma-buf -> same handle, Mesa's bo_list find_and_ref shares the dt)
 *   drmIoctl(render_fd, DRM_IOCTL_MODE_MAP_DUMB, {handle}) -> offset
 *   mmap(render_fd, size, PROT_READ, MAP_SHARED, offset)
 * and on resource teardown:
 *   drmCloseBufferHandle(fd, handle)
 *
 * On the reference kernel handles are PER-FILE: gralloc closing its handle
 * for a buffer never invalidates render_service's handle. On Rux the gems
 * table is GLOBAL and gem_close removes the entry unconditionally, so a
 * close from ANY importer kills the handle for EVERYONE.
 *
 * This probe reproduces that with two drm fds in one process:
 *   P1: open card0 + renderD128 x2
 *   P2: CREATE_DUMB on card0 -> HANDLE_TO_FD -> prime fd
 *   P3: FD_TO_HANDLE on renderA -> H1; lseek(prime_fd) -> size
 *   P4: MAP_DUMB(H1) -> offset; mmap PROT_READ -> read check   (llvmpipe map)
 *   P5: FD_TO_HANDLE on renderB -> H2 (global table dedupes -> H2 == H1)
 *   P6: GEM_CLOSE(H1) on renderA      (the "other importer" closes)
 *   P7: MAP_DUMB(H2) on renderB       (reference: OK; global-table: EINVAL)
 *   P8: FD_TO_HANDLE on renderB again -> re-register -> MAP_DUMB -> mmap RW
 *   P9: PROT_READ-only mmap retry at the H2 offset (prot-policy check)
 * Every step prints one short GLMPROBE line to stderr (kernel stderr tap
 * carries them to serial).
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/ioctl.h>
#include <errno.h>
#include <time.h>

#define DRM_IOCTL_BASE 'd'
#define _IOC_NR_SH 0
static unsigned long ioc(unsigned dir, unsigned char nr, unsigned size)
{
    return (dir << 30) | (size << 16) | (DRM_IOCTL_BASE << 8) | nr;
}
/* struct drm_mode_create_dumb: u32 h,w,bpp,flags,handle,pitch; u64 size = 32.
 * nr 0xb2 per the OH reference kernels (5.10/6.6 drm.h — the ABI the image's
 * libdrm/gralloc actually speaks; upstream-mainline's 0xb4 is DESTROY_DUMB
 * here). MATCHES the Rux kernel table. */
#define DRM_IOCTL_MODE_CREATE_DUMB ioc(3, 0xb2, 32)
/* struct drm_prime_handle: s32 fd; u32 flags; u32 handle = 12 */
#define DRM_IOCTL_PRIME_HANDLE_TO_FD ioc(3, 0x2d, 12)
#define DRM_IOCTL_PRIME_FD_TO_HANDLE ioc(3, 0x2e, 12)
/* struct drm_mode_map_dumb: u32 handle; u32 pad; u64 offset = 16 */
#define DRM_IOCTL_MODE_MAP_DUMB ioc(3, 0xb3, 16)
/* struct drm_gem_close: u32 handle; u32 pad = 8 */
#define DRM_IOCTL_GEM_CLOSE ioc(1, 0x09, 8)

struct create_dumb {
    uint32_t height, width, bpp, flags, handle, pitch;
    uint64_t size;
};
/* OH libdrm layout (third_party/libdrm/include/drm/drm.h): handle, flags,
 * fd — the SWAP of upstream's {fd, flags, handle}. The Rux kernel matches
 * this (reads handle@0 for TO_FD, fd@8 for FD_TO_HANDLE). */
struct prime_handle {
    uint32_t handle;
    uint32_t flags;
    int32_t fd;
};
struct map_dumb {
    uint32_t handle, pad;
    uint64_t offset;
};
struct gem_close {
    uint32_t handle, pad;
};

static void P(const char *step, int ok, long val)
{
    fprintf(stderr, "GLMPROBE %s %s val=%ld errno=%d\n", step, ok ? "ok" : "FAIL", val, errno);
    fflush(stderr);
}

static int wait_node(const char *path, int secs)
{
    for (int i = 0; i < secs * 10; i++) {
        int fd = open(path, O_RDWR);
        if (fd >= 0)
            return fd;
        struct timespec ts = {0, 100 * 1000 * 1000};
        nanosleep(&ts, NULL);
    }
    return -1;
}

int main(void)
{
    int fd_card = wait_node("/dev/dri/card0", 180);
    P("P1a-card0", fd_card >= 0, fd_card);
    if (fd_card < 0)
        return 1;
    int fd_a = wait_node("/dev/dri/renderD128", 30);
    if (fd_a < 0) {
        P("P1b-renderA-missing-use-card0", 1, 0);
        fd_a = dup(fd_card);
    }
    P("P1b-renderA", fd_a >= 0, fd_a);
    int fd_b = wait_node("/dev/dri/renderD128", 30);
    if (fd_b < 0)
        fd_b = dup(fd_card);
    P("P1c-renderB", fd_b >= 0, fd_b);

    /* P2: create a dumb buffer on card0 and export it as a prime fd */
    struct create_dumb cd;
    memset(&cd, 0, sizeof cd);
    cd.width = 720;
    cd.height = 1280;
    cd.bpp = 32;
    cd.flags = 0;
    int ret = ioctl(fd_card, DRM_IOCTL_MODE_CREATE_DUMB, &cd);
    P("P2a-create-dumb", ret == 0, ret == 0 ? (long)cd.handle : ret);
    if (ret)
        return 1;

    struct prime_handle ph;
    memset(&ph, 0, sizeof ph);
    ph.handle = cd.handle;
    ret = ioctl(fd_card, DRM_IOCTL_PRIME_HANDLE_TO_FD, &ph);
    P("P2b-handle-to-fd", ret == 0, ret == 0 ? (long)ph.fd : ret);
    if (ret)
        return 1;
    int prime_fd = ph.fd;

    /* P3: import on renderA */
    struct prime_handle ih;
    memset(&ih, 0, sizeof ih);
    ih.fd = prime_fd;
    ret = ioctl(fd_a, DRM_IOCTL_PRIME_FD_TO_HANDLE, &ih);
    P("P3-fd-to-handle-A", ret == 0, ret == 0 ? (long)ih.handle : ret);
    if (ret)
        return 1;
    uint32_t h1 = ih.handle;

    off_t sz = lseek(prime_fd, 0, SEEK_END);
    P("P3-lseek-size", sz > 0, (long)sz);
    lseek(prime_fd, 0, SEEK_SET);

    /* P4: the llvmpipe map: MAP_DUMB + PROT_READ mmap */
    struct map_dumb md;
    memset(&md, 0, sizeof md);
    md.handle = h1;
    ret = ioctl(fd_a, DRM_IOCTL_MODE_MAP_DUMB, &md);
    P("P4a-map-dumb-H1", ret == 0, ret == 0 ? (long)md.offset : ret);
    if (ret == 0) {
        void *m = mmap(NULL, sz, PROT_READ, MAP_SHARED, fd_a, md.offset);
        P("P4b-mmap-ro", m != MAP_FAILED, (long)(m == MAP_FAILED ? -1 : (intptr_t)m));
        if (m != MAP_FAILED) {
            unsigned sum = 0;
            const volatile uint32_t *p = m;
            for (unsigned i = 0; i < 256; i++)
                sum += p[i];
            P("P4c-read-ro", 1, sum);
            munmap(m, sz);
        }
    }

    /* P5: import the SAME dma-buf on renderB (global dedupe check) */
    struct prime_handle ih2;
    memset(&ih2, 0, sizeof ih2);
    ih2.fd = prime_fd;
    ret = ioctl(fd_b, DRM_IOCTL_PRIME_FD_TO_HANDLE, &ih2);
    P("P5-fd-to-handle-B", ret == 0, ret == 0 ? (long)ih2.handle : ret);
    if (ret)
        return 1;
    uint32_t h2 = ih2.handle;
    fprintf(stderr, "GLMPROBE P5-dedupe %s h1=%u h2=%u\n",
            h1 == h2 ? "SAME-HANDLE" : "DIFFERENT-HANDLE", h1, h2);
    fflush(stderr);

    /* P6: the other importer closes its handle (gralloc recycle class) */
    struct gem_close gc;
    memset(&gc, 0, sizeof gc);
    gc.handle = h1;
    ret = ioctl(fd_a, DRM_IOCTL_GEM_CLOSE, &gc);
    P("P6-close-H1-on-A", ret == 0, ret);

    /* P7: map on renderB with ITS handle — reference kernel: per-file
     * namespaces make this immune; global table: EINVAL */
    memset(&md, 0, sizeof md);
    md.handle = h2;
    ret = ioctl(fd_b, DRM_IOCTL_MODE_MAP_DUMB, &md);
    P("P7-map-dumb-H2-after-foreign-close", ret == 0, ret == 0 ? (long)md.offset : ret);

    /* P8: re-import on renderB (the RS import#18 pattern) then map */
    struct prime_handle ih3;
    memset(&ih3, 0, sizeof ih3);
    ih3.fd = prime_fd;
    ret = ioctl(fd_b, DRM_IOCTL_PRIME_FD_TO_HANDLE, &ih3);
    P("P8a-fd-to-handle-B-again", ret == 0, ret == 0 ? (long)ih3.handle : ret);
    if (ret == 0) {
        memset(&md, 0, sizeof md);
        md.handle = ih3.handle;
        ret = ioctl(fd_b, DRM_IOCTL_MODE_MAP_DUMB, &md);
        P("P8b-map-dumb-H3", ret == 0, ret == 0 ? (long)md.offset : ret);
        if (ret == 0) {
            void *m = mmap(NULL, sz, PROT_READ | PROT_WRITE, MAP_SHARED, fd_b, md.offset);
            P("P8c-mmap-rw", m != MAP_FAILED, (long)(m == MAP_FAILED ? -1 : (intptr_t)m));
            if (m != MAP_FAILED) {
                ((volatile uint32_t *)m)[0] = 0xff00ff00;
                P("P8d-write-rw", 1, 0);
                munmap(m, sz);
            }
        }
    }

    /* P9: PROT_READ-only retry at the surviving handle's offset */
    if (fd_a >= 0) {
        struct prime_handle ih4;
        memset(&ih4, 0, sizeof ih4);
        ih4.fd = prime_fd;
        if (ioctl(fd_a, DRM_IOCTL_PRIME_FD_TO_HANDLE, &ih4) == 0) {
            memset(&md, 0, sizeof md);
            md.handle = ih4.handle;
            if (ioctl(fd_a, DRM_IOCTL_MODE_MAP_DUMB, &md) == 0) {
                void *m = mmap(NULL, sz, PROT_READ, MAP_SHARED, fd_a, md.offset);
                P("P9-mmap-ro-second-fd", m != MAP_FAILED, (long)(m == MAP_FAILED ? -1 : (intptr_t)m));
                if (m != MAP_FAILED)
                    munmap(m, sz);
            }
        }
    }

    fprintf(stderr, "GLMPROBE done\n");
    fflush(stderr);
    return 0;
}
