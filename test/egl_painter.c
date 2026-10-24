/*
 * egl_painter.c — the bootanimation-seat painter (ui-sprint "pixels today").
 *
 * Runs in the REAL OH boot from the bootanimation service seat (init
 * starts it uid=graphics after render_service/composer are up — it
 * replaces /system/bin/bootanimation on this boot's private system
 * image copy). Two phases:
 *
 *   1. wait: the graphics cohort (RS, composer_host, allocator_host)
 *      needs ~5-6 min under KVM to settle; poll /dev/dri/card0 +
 *      sleep in slices so early starts are tolerated.
 *   2. paint: the proven B ladder (CREATE_DUMB -> MAP_DUMB -> mmap ->
 *      ADDFB2 -> MODE_ATOMIC flip), re-painting a moving color field
 *      every 400 ms so shotmon catches a live screen, forever.
 *
 * The A ladder (EGL + llvmpipe) also runs once for the record — its
 * result prints on serial (the real-boot RS EGL failure is a separate
 * hunt; this painter does not need EGL for pixels).
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

#define DRM_IOCTL_MODE_GETRESOURCES 0xc04064a0u
struct drm_card_res_ {
    uint64_t fb_id_ptr, crtc_id_ptr, connector_id_ptr, encoder_id_ptr;
    uint32_t count_fbs, count_crtcs, count_connectors, count_encoders;
    uint32_t min_width, max_width, min_height, max_height;
};
#define DRM_IOCTL_MODE_CREATE_DUMB 0xc02064b2u
struct drm_mode_create_dumb_ {
    uint32_t height, width, bpp, flags;
    uint32_t handle, pitch;
    uint64_t size;
};
#define DRM_IOCTL_MODE_MAP_DUMB 0xc01064b3u
struct drm_mode_map_dumb_ { uint32_t handle; uint32_t pad; uint64_t offset; };
#define DRM_IOCTL_MODE_ADDFB2 0xc06864b8u
struct drm_mode_fb_cmd2_ {
    uint32_t fb_id, width, height;
    uint32_t pixel_format, flags;
    uint32_t handles[4], pitches[4], offsets[4];
};
struct drm_mode_atomic_oh_ {
    uint32_t flags, count_objs;
    uint64_t objs_ptr, counts_ptr, props_ptr, prop_values_ptr, user_data;
};
#define DRM_IOCTL_MODE_ATOMIC 0xc03864bcu

#define W 720
#define H 1280

static int flip_fd = -1;
static uint8_t *fb = NULL;
static uint32_t fb_pitch;
static uint32_t fb_id;
static uint32_t plane_id = 43;

static int setup_flip(void)
{
    int fd = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
    if (fd < 0) return -1;
    struct drm_mode_create_dumb_ dumb = { .height = H, .width = W, .bpp = 32 };
    if (ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &dumb) != 0) { close(fd); return -1; }
    struct drm_mode_map_dumb_ md = { .handle = dumb.handle };
    if (ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &md) != 0) { close(fd); return -1; }
    uint8_t *p = mmap(NULL, dumb.size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, md.offset);
    if (p == MAP_FAILED) { close(fd); return -1; }
    struct drm_mode_fb_cmd2_ fbc;
    memset(&fbc, 0, sizeof fbc);
    fbc.width = W; fbc.height = H;
    fbc.pixel_format = 0x34325258u; /* XRGB8888 */
    fbc.handles[0] = dumb.handle; fbc.pitches[0] = dumb.pitch;
    if (ioctl(fd, DRM_IOCTL_MODE_ADDFB2, &fbc) != 0 || !fbc.fb_id) { close(fd); return -1; }
    flip_fd = fd; fb = p; fb_pitch = dumb.pitch; fb_id = fbc.fb_id;
    return 0;
}

static int flip(void)
{
    uint32_t objs[1] = { plane_id };
    uint32_t counts[1] = { 1 };
    uint32_t props[1] = { 9 }; /* PROP_PLANE_FB_ID */
    uint64_t values[1] = { fb_id };
    struct drm_mode_atomic_oh_ atom = {
        .flags = 0, .count_objs = 1,
        .objs_ptr = (uintptr_t)objs, .counts_ptr = (uintptr_t)counts,
        .props_ptr = (uintptr_t)props, .prop_values_ptr = (uintptr_t)values,
        .user_data = 0,
    };
    return ioctl(flip_fd, DRM_IOCTL_MODE_ATOMIC, &atom);
}

static uint32_t frame;

static void paint(void)
{
    uint32_t base = (frame * 9) & 0xff;
    for (uint32_t y = 0; y < H; y++) {
        uint32_t band = ((y + frame * 13) / 64) & 3;
        uint32_t r = band == 0 ? base : 0x20;
        uint32_t g = band == 1 ? base : 0x20;
        uint32_t b = band == 2 ? base : 0x60;
        uint32_t c = 0xff000000u | (r << 16) | (g << 8) | b;
        uint32_t *row = (uint32_t *)(fb + (size_t)y * fb_pitch);
        for (uint32_t x = 0; x < W; x++)
            row[x] = ((x / 45 + frame) & 7) == 0 ? 0xffffffffu : c;
    }
    frame++;
}

static void try_egl(void)
{
    void *h = dlopen("/vendor/lib64/chipsetsdk/libEGL.so", RTLD_NOW | RTLD_LOCAL);
    if (!h) {
        printf("egl_painter: dlopen libEGL failed: %s\n", dlerror());
        return;
    }
    void *(*eglGetDisplay)(void *) = dlsym(h, "eglGetDisplay");
    unsigned (*eglInitialize)(void *, int *, int *) = dlsym(h, "eglInitialize");
    if (!eglGetDisplay || !eglInitialize) return;
    void *dpy = eglGetDisplay((void *)0);
    int maj = 0, min = 0;
    if (dpy && eglInitialize(dpy, &maj, &min))
        printf("egl_painter: eglInitialize OK %d.%d\n", maj, min);
    else
        printf("egl_painter: eglInitialize FAILED (rs-shape repro)\n");
}

int main(void)
{
    printf("egl_painter: start (bootanimation seat)\n");
    /* wait for the graphics cohort: card0 is registered from boot; the
     * RS/composer settle takes minutes under KVM. 8 min max. */
    for (int i = 0; i < 96; i++) {
        int fd = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
        if (fd >= 0) { close(fd); break; }
        usleep(500000);
    }
    sleep(240); /* let RS/composer settle first */
    try_egl();
    if (setup_flip() != 0) {
        printf("egl_painter: setup_flip FAILED (%s)\n", strerror(errno));
        for (;;) sleep(60);
    }
    printf("egl_painter: flip loop begin (fb %u)\n", fb_id);
    for (;;) {
        paint();
        if (flip() != 0)
            printf("egl_painter: flip errno=%d\n", errno);
        usleep(400000);
    }
    return 0;
}
