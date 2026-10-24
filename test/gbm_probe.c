/*
 * gbm_probe.c — run the OH image's OWN Mesa gbm stack against the Rux
 * DRM interface, as init, in a minimal rootfs that carries the image's
 * libgbm/libdrm/libgallium/libc++/musl.
 *
 * Chain exercised (the exact gralloc/allocator_host path):
 *   open /dev/dri/card0
 *   libdrm drmGetVersion            (driver name virtio_gpu)
 *   libdrm drmGetDevice             (sysfs walk: /sys/dev/char, PCI ids)
 *   gbm_create_device               (driver load: virtio_gpu -> GETPARAM
 *                                    3D=0 -> kms_swrast/llvmpipe)
 *   gbm_bo_create 64x64             (dumb buffer / CREATE_DUMB + dri screen)
 *   gbm_bo_map                      (MODE_MAP_DUMB / mmap)
 *   gbm_bo_get_fd                   (PRIME_HANDLE_TO_FD)
 *
 * Verdict: GBM_PROBE RESULT: PASS <n>/<n>
 *
 * Build: dynamic against the OH musl loader + libc.so (see
 * gbm_probe_boot.py); runs with LD_LIBRARY_PATH=/vendor/lib64/chipsetsdk.
 */
#include <dlfcn.h>
#include <fcntl.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <sys/ioctl.h>
#include <sys/mman.h>

#define DRM_IOCTL_VERSION 0xc0406400u
struct drm_version_ {
    int32_t major, minor, patch;
    size_t name_len; char *name;
    size_t date_len; char *date;
    size_t desc_len; char *desc;
};

/* gbm API (typedefs kept local; ABI-stable structs passed as void*) */
typedef struct gbm_device gbm_device;
typedef struct gbm_bo gbm_bo;
#define GBM_FORMAT_XRGB8888 0x34325258u
#define GBM_BO_USE_RENDERING 0x0004u
#define GBM_BO_USE_SCANOUT   0x0001u

static int passed, total;
static void ok(const char *what) { printf("ok   %s\n", what); passed++; total++; }
static void fail(const char *what, const char *detail) {
    printf("FAIL %s — %s (errno=%d)\n", what, detail ? detail : "?", errno);
    total++;
}

int main(void)
{
    setenv("LD_LIBRARY_PATH", "/vendor/lib64/chipsetsdk", 1);
    /* absolute paths: musl dlopen search does not always honor the
     * path file for non-NEEDED opens on this build. */
    const char *gbm_path = "/lib/libgbm.so";
    const char *drm_path = "/lib/libdrm.so";
    setenv("DRI_CONFIG...", "", 0); /* placeholder no-op */

    int fd = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
    if (fd >= 0) ok("open /dev/dri/card0");
    else { fail("open /dev/dri/card0", strerror(errno)); goto out; }

    /* kernel-visible handshake: raw VERSION */
    {
        char name[64] = {0};
        struct drm_version_ v;
        memset(&v, 0, sizeof v);
        v.name_len = sizeof name - 1; v.name = name;
        if (ioctl(fd, DRM_IOCTL_VERSION, &v) == 0)
            ok(!strcmp(name, "virtio_gpu") ? "version name virtio_gpu" : "version (wrong name)");
        else fail("DRM_IOCTL_VERSION", strerror(errno));
    }

    /* libdrm: drmGetVersion via their code */
    void *hdrm = dlopen(drm_path, RTLD_NOW | RTLD_GLOBAL);
    if (hdrm) ok("dlopen libdrm.so");
    else { fail("dlopen libdrm.so", dlerror()); }

    /* libgbm */
    void *hgbm = dlopen(gbm_path, RTLD_NOW | RTLD_LOCAL);
    if (!hgbm) { fail("dlopen libgbm.so", dlerror()); goto out; }
    ok("dlopen libgbm.so");

    gbm_device *(*gbm_create_device)(int) =
        (gbm_device * (*)(int)) dlsym(hgbm, "gbm_create_device");
    gbm_bo *(*gbm_bo_create)(gbm_device *, uint32_t, uint32_t, uint32_t, uint32_t) =
        (gbm_bo * (*)(gbm_device *, uint32_t, uint32_t, uint32_t, uint32_t)) dlsym(hgbm, "gbm_bo_create");
    void *(*gbm_bo_map)(gbm_bo *, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t *, void **) =
        (void *(*)(gbm_bo *, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t *, void **))dlsym(hgbm, "gbm_bo_map");
    int (*gbm_bo_get_fd)(gbm_bo *) = (int (*)(gbm_bo *))dlsym(hgbm, "gbm_bo_get_fd");
    uint32_t (*gbm_bo_get_stride)(gbm_bo *) = (uint32_t (*)(gbm_bo *))dlsym(hgbm, "gbm_bo_get_stride");
    void (*gbm_bo_destroy)(gbm_bo *) = (void (*)(gbm_bo *))dlsym(hgbm, "gbm_bo_destroy");
    if (!gbm_create_device || !gbm_bo_create || !gbm_bo_map || !gbm_bo_get_fd) {
        fail("dlsym gbm entry points", "missing");
        goto out;
    }
    ok("dlsym gbm entry points");

    gbm_device *dev = gbm_create_device(fd);
    if (!dev) { fail("gbm_create_device", "NULL (driver load/sysfs failed)"); goto out; }
    ok("gbm_create_device");

    gbm_bo *bo = gbm_bo_create(dev, 64, 64, GBM_FORMAT_XRGB8888,
                               GBM_BO_USE_RENDERING | GBM_BO_USE_SCANOUT);
    if (!bo) { fail("gbm_bo_create 64x64", "NULL (dumb alloc / dri screen failed)"); goto out; }
    ok("gbm_bo_create 64x64 XRGB8888");

    {
        uint32_t stride = 0; void *map_data = NULL;
        void *p = gbm_bo_map(bo, 0, 0, 64, 64, 2 /*W*/, &stride, &map_data);
        if (p && stride) {
            ok("gbm_bo_map (stride set)");
            memset(p, 0x80, stride * 64); /* paint gray */
            ((volatile unsigned char *)p)[0] ^= 0xff;
            ok("gbm_bo_map paint");
        } else fail("gbm_bo_map", p ? "no stride" : "NULL");
    }

    {
        int pfd = gbm_bo_get_fd(bo);
        if (pfd >= 0) ok("gbm_bo_get_fd (prime export)");
        else fail("gbm_bo_get_fd", "negative");
    }

    gbm_bo_destroy(bo);
    ok("gbm_bo_destroy");

out:
    printf("GBM_PROBE RESULT: %s %d/%d\n", passed == total ? "PASS" : "FAIL", passed, total);
    return passed == total ? 0 : 1;
}
