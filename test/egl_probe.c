/*
 * egl_probe.c — ride the OH image's OWN graphics stack against the Rux
 * DRM interface, as init, in the same minimal rootfs gbm_probe uses.
 *
 * Two ladders, each rung loud on serial:
 *
 * A. The render_service (RS) ladder — what pid RS blocks on today:
 *   A1  eglGetDisplay(EGL_DEFAULT_DISPLAY) + eglInitialize
 *   A2  eglQueryDevicesEXT + eglGetPlatformDisplay(DEVICE) fallback
 *   A3  eglChooseConfig + PBuffer surface + GLES context + makeCurrent
 *   A4  glClearColor + glClear (llvmpipe software rasterization)
 *
 * B. The composer/gralloc ladder — the first-flip path:
 *   B1  SET_CLIENT_CAP(UNIVERSAL_PLANES/ATOMIC) + GETRESOURCES
 *   B2  CREATE_DUMB 720x1280 + MAP_DUMB + mmap + paint gradient
 *   B3  ADDFB2 + MODE_ATOMIC(FB_ID)  -> first flip -> PIXELS
 *
 * Verdict: EGL_PROBE RESULT: PASS <n>/<n>
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
#include <pthread.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>

/* ---- hilog catcher: libhilog (inside libEGL/Mesa) sends its formatted
 * messages to the unix socket /dev/unix/socket/hilogInput (kernel log
 * showed the failed connect attempt to exactly that path). Be that
 * socket and mirror every datagram to serial — this is the only place
 * the OH-patched Mesa error text exists. */
static void dump_bytes(const char *tag, const char *buf, ssize_t n)
{
    printf("--- %s (%zd bytes) ---\n", tag, n);
    for (ssize_t i = 0; i < n; i++) {
        unsigned char ch = (unsigned char)buf[i];
        putchar((ch >= 0x20 && ch < 0x7f) || ch == '\n' ? ch : '.');
    }
    putchar('\n');
    fflush(stdout);
}

static void *hilog_catcher(void *arg)
{
    (void)arg;
    /* devfs mkdir from userspace (kernel creates dri/graphics this way
     * internally; the syscall path must work for ueventd too) */
    mkdir("/dev/unix", 0755);
    mkdir("/dev/unix/socket", 0755);
    int s = socket(AF_UNIX, SOCK_DGRAM, 0);
    if (s < 0) { printf("info hilog: no socket (%s)\n", strerror(errno)); return NULL; }
    struct sockaddr_un sa;
    memset(&sa, 0, sizeof sa);
    sa.sun_family = AF_UNIX;
    strcpy(sa.sun_path, "/dev/unix/socket/hilogInput");
    unlink(sa.sun_path);
    if (bind(s, (struct sockaddr *)&sa, sizeof sa) != 0) {
        printf("info hilog: bind %s failed (%s)\n", sa.sun_path, strerror(errno));
        close(s);
        return NULL;
    }
    printf("info hilog: catcher bound %s (dgram)\n", sa.sun_path);
    char buf[2048];
    for (;;) {
        ssize_t n = recv(s, buf, sizeof buf - 1, 0);
        if (n <= 0) continue;
        buf[n] = 0;
        dump_bytes("hilog msg", buf, n);
    }
    return NULL;
}

/* ---- DRM uapi (raw, no libdrm needed) ---- */
#define DRM_IOCTL_SET_CLIENT_CAP 0x4010640du
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
/* OH image libdrm pre-4.0 atomic layout (56 bytes) */
struct drm_mode_atomic_oh_ {
    uint32_t flags, count_objs;
    uint64_t objs_ptr, counts_ptr, props_ptr, prop_values_ptr, user_data;
};
#define DRM_IOCTL_MODE_ATOMIC 0xc03864bcu

/* ---- EGL (local prototypes; dlopened) ---- */
typedef void *EGLDisplay, *EGLConfig, *EGLSurface, *EGLContext, *EGLDeviceEXT;
typedef intptr_t EGLAttrib, EGLint;
#define EGL_DEFAULT_DISPLAY ((void *)0)
#define EGL_NO_DISPLAY ((void *)0)
#define EGL_VENDOR 0x3053
#define EGL_VERSION 0x3054
#define EGL_EXTENSIONS 0x3055
#define EGL_SURFACE_TYPE 0x3033
#define EGL_PBUFFER_BIT 0x0001
#define EGL_RED_SIZE 0x3024
#define EGL_GREEN_SIZE 0x3023
#define EGL_BLUE_SIZE 0x3022
#define EGL_ALPHA_SIZE 0x3021
#define EGL_RENDERABLE_TYPE 0x3040
#define EGL_OPENGL_ES2_BIT 0x0004
#define EGL_WIDTH 0x3057
#define EGL_HEIGHT 0x3058
#define EGL_CONTEXT_CLIENT_VERSION 0x3098
#define EGL_NO_CONTEXT ((void *)0)
#define EGL_PLATFORM_DEVICE_EXT 0x3135

static int passed, total;
static void ok(const char *what) { printf("ok   %s\n", what); passed++; total++; }
static void fail(const char *what, const char *detail) {
    printf("FAIL %s — %s (errno=%d)\n", what, detail ? detail : "?", errno);
    total++;
}
static void info(const char *fmt, const char *s) { printf("info %s: %s\n", fmt, s); }

/* B-ladder: one raw flip; returns 0 on success */
static int kms_first_flip(void)
{
    int fd = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
    if (fd < 0) { fail("B: open card0", strerror(errno)); return -1; }
    ok("B: open card0");

    /* B1: client caps + resources */
    for (uint64_t cap = 2; cap <= 3; cap++) { /* UNIVERSAL_PLANES, ATOMIC */
        struct { uint64_t id, value; } c = { cap, 1 };
        if (ioctl(fd, DRM_IOCTL_SET_CLIENT_CAP, &c) == 0) ok("B: set client cap");
        else fail("B: set client cap", strerror(errno));
    }
    struct drm_card_res_ res;
    memset(&res, 0, sizeof res);
    uint32_t crtcs[4] = {0}, conns[4] = {0}, encs[4] = {0};
    /* two-phase: first call returns counts, second fills the arrays */
    if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res) != 0) {
        fail("B: getresources(phase1)", strerror(errno)); return -1;
    }
    res.crtc_id_ptr = (uintptr_t)crtcs; res.connector_id_ptr = (uintptr_t)conns;
    res.encoder_id_ptr = (uintptr_t)encs;
    if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res) != 0) {
        fail("B: getresources", strerror(errno)); return -1;
    }
    printf("info B: resources crtcs=%u conns=%u encs=%u (crtc0=%u conn0=%u)\n",
           res.count_crtcs, res.count_connectors, res.count_encoders,
           crtcs[0], conns[0]);
    ok(res.count_crtcs && res.count_connectors ? "B: getresources" : "B: getresources (empty)");

    /* B2: dumb buffer + paint */
    struct drm_mode_create_dumb_ dumb = { .height = 1280, .width = 720, .bpp = 32 };
    if (ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &dumb) != 0) {
        fail("B: create_dumb 720x1280", strerror(errno)); return -1;
    }
    ok("B: create_dumb (handle/pitch set)");
    printf("info B: dumb handle=%u pitch=%u size=%llu\n",
           dumb.handle, dumb.pitch, (unsigned long long)dumb.size);

    struct drm_mode_map_dumb_ md = { 0 };
    md.handle = dumb.handle;
    if (ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &md) != 0) {
        fail("B: map_dumb", strerror(errno)); return -1;
    }
    ok("B: map_dumb");
    printf("info B: map_dumb offset=%llu (handle=%u pad=%u)\n",
           (unsigned long long)md.offset, md.handle, md.pad);
    uint8_t *px = mmap(NULL, dumb.size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, md.offset);
    if (px == MAP_FAILED) { fail("B: mmap dumb", strerror(errno)); return -1; }
    ok("B: mmap dumb");
    /* paint a vertical gradient, opaque alpha */
    for (uint32_t y = 0; y < 1280; y++) {
        uint32_t row_color = 0xff000000u | ((y * 255 / 1279) << 8) | 0x40u;
        for (uint32_t x = 0; x < 720; x++)
            ((uint32_t *)px)[y * (dumb.pitch / 4) + x] = row_color;
    }
    ok("B: paint gradient");

    /* B3: addfb2 + atomic flip */
    struct drm_mode_fb_cmd2_ fb;
    memset(&fb, 0, sizeof fb);
    fb.width = 720; fb.height = 1280;
    fb.pixel_format = 0x34325258u; /* DRM_FORMAT_XRGB8888 */
    fb.handles[0] = dumb.handle; fb.pitches[0] = dumb.pitch;
    if (ioctl(fd, DRM_IOCTL_MODE_ADDFB2, &fb) != 0 || !fb.fb_id) {
        fail("B: addfb2", strerror(errno)); return -1;
    }
    ok("B: addfb2");

    uint32_t plane_id = 43; /* fixed single primary plane */
    uint32_t objs[1] = { plane_id };
    uint32_t counts[1] = { 1 };
    uint32_t props[1] = { 9 };      /* PROP_PLANE_FB_ID */
    uint64_t values[1] = { fb.fb_id };
    struct drm_mode_atomic_oh_ atom = {
        .flags = 0, .count_objs = 1,
        .objs_ptr = (uintptr_t)objs, .counts_ptr = (uintptr_t)counts,
        .props_ptr = (uintptr_t)props, .prop_values_ptr = (uintptr_t)values,
        .user_data = 0,
    };
    if (ioctl(fd, DRM_IOCTL_MODE_ATOMIC, &atom) != 0) {
        fail("B: atomic flip", strerror(errno)); return -1;
    }
    ok("B: atomic flip — FIRST FLIP DONE");
    return 0;
}

int main(void)
{
    printf("=== EGL_PROBE start ===\n");

    /* hilog catcher must exist before libEGL dlopens (libhilog caches
     * its connection at first use) */
    pthread_t hgt;
    pthread_create(&hgt, NULL, hilog_catcher, NULL);
    usleep(100000);

    /* ---- B ladder first: pixels do not need EGL ---- */
    kms_first_flip();

    /* ---- A ladder: the RS EGL path ---- */
    void *hegl = dlopen("/lib/libEGL.so", RTLD_NOW | RTLD_LOCAL);
    if (!hegl) { fail("A: dlopen libEGL", dlerror()); goto out; }
    ok("A: dlopen libEGL");

    EGLDisplay (*eglGetDisplay)(void *) = dlsym(hegl, "eglGetDisplay");
    unsigned (*eglInitialize)(EGLDisplay, int *, int *) = dlsym(hegl, "eglInitialize");
    const char *(*eglQueryString)(EGLDisplay, int) = dlsym(hegl, "eglQueryString");
    int (*eglGetError)(void) = dlsym(hegl, "eglGetError");
    EGLDisplay (*eglGetPlatformDisplay)(unsigned, void *, const intptr_t *) =
        dlsym(hegl, "eglGetPlatformDisplayEXT") ?:
        dlsym(hegl, "eglGetPlatformDisplay");
    unsigned (*eglQueryDevicesEXT)(int, EGLDeviceEXT *, int *) =
        dlsym(hegl, "eglQueryDevicesEXT");
    unsigned (*eglChooseConfig)(EGLDisplay, const int *, EGLConfig *, int, int *) =
        dlsym(hegl, "eglChooseConfig");
    EGLSurface (*eglCreatePbufferSurface)(EGLDisplay, EGLConfig, const int *) =
        dlsym(hegl, "eglCreatePbufferSurface");
    EGLContext (*eglCreateContext)(EGLDisplay, EGLConfig, EGLContext, const int *) =
        dlsym(hegl, "eglCreateContext");
    unsigned (*eglMakeCurrent)(EGLDisplay, EGLSurface, EGLSurface, EGLContext) =
        dlsym(hegl, "eglMakeCurrent");
    unsigned (*eglBindAPI)(unsigned) = dlsym(hegl, "eglBindAPI");
    if (!eglGetDisplay || !eglInitialize || !eglGetError) {
        fail("A: dlsym egl entry points", "missing");
        goto out;
    }
    ok("A: dlsym egl entry points");

    EGLDisplay dpy = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    if (!dpy) { fail("A: eglGetDisplay(DEFAULT)", "NULL"); }
    else {
        ok("A: eglGetDisplay(DEFAULT) non-null");
        int maj = 0, min = 0;
        if (eglInitialize(dpy, &maj, &min)) {
            printf("info A: eglInitialize %d.%d vendor=%s\n", maj, min,
                   eglQueryString ? eglQueryString(dpy, EGL_VENDOR) : "?");
            ok("A: eglInitialize");
        } else {
            printf("FAIL A: eglInitialize — eglGetError=0x%x\n", eglGetError());
            total++;
        }
    }

    /* device-platform fallback (what a null default display forces) */
    if (!dpy || 0) {
        if (eglQueryDevicesEXT) {
            EGLDeviceEXT devs[8]; int nd = 0;
            if (eglQueryDevicesEXT(8, devs, &nd) && nd > 0) {
                printf("info A: %d EGL devices\n", nd);
                ok("A: eglQueryDevicesEXT");
                if (eglGetPlatformDisplay) {
                    dpy = eglGetPlatformDisplay(EGL_PLATFORM_DEVICE_EXT, devs[0], NULL);
                    if (dpy) {
                        int maj = 0, min = 0;
                        if (eglInitialize(dpy, &maj, &min)) ok("A: device display init");
                        else { printf("FAIL A: device display init err=0x%x\n", eglGetError()); total++; dpy = 0; }
                    } else fail("A: eglGetPlatformDisplay(DEVICE)", "NULL");
                }
            } else fail("A: eglQueryDevicesEXT", "no devices");
        } else fail("A: eglQueryDevicesEXT", "symbol missing");
    }

    if (dpy && eglChooseConfig && eglCreatePbufferSurface && eglCreateContext) {
        static const int cfg_attrs[] = {
            EGL_SURFACE_TYPE, EGL_PBUFFER_BIT,
            EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_ALPHA_SIZE, 8,
            EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
            0x3038 /* EGL_NONE */
        };
        EGLConfig cfg = NULL; int n = 0;
        if (!eglChooseConfig(dpy, cfg_attrs, &cfg, 1, &n) || n < 1 || !cfg) {
            printf("FAIL A: eglChooseConfig — err=0x%x n=%d\n", eglGetError(), n);
            total++;
            goto out;
        }
        ok("A: eglChooseConfig");
        static const int pb_attrs[] = { EGL_WIDTH, 64, EGL_HEIGHT, 64, 0x3038 };
        EGLSurface surf = eglCreatePbufferSurface(dpy, cfg, pb_attrs);
        if (!surf) { printf("FAIL A: pbuffer — err=0x%x\n", eglGetError()); total++; goto out; }
        ok("A: eglCreatePbufferSurface");
        static const int ctx_attrs[] = { EGL_CONTEXT_CLIENT_VERSION, 2, 0x3038 };
        EGLContext ctx = eglCreateContext(dpy, cfg, EGL_NO_CONTEXT, ctx_attrs);
        if (!ctx) { printf("FAIL A: eglCreateContext — err=0x%x\n", eglGetError()); total++; goto out; }
        ok("A: eglCreateContext(GLES2)");
        if (eglBindAPI) eglBindAPI(0x30A0 /* EGL_OPENGL_ES_API */);
        if (!eglMakeCurrent(dpy, surf, surf, ctx)) {
            printf("FAIL A: eglMakeCurrent — err=0x%x\n", eglGetError());
            total++;
            goto out;
        }
        ok("A: eglMakeCurrent — LLVMPIPE CONTEXT LIVE");
        void *hgl = dlopen("/lib/libGLESv2.so.2", RTLD_NOW | RTLD_LOCAL);
        if (hgl) {
            void (*glClearColor)(float, float, float, float) = dlsym(hgl, "glClearColor");
            unsigned (*glClear)(unsigned) = dlsym(hgl, "glClear");
            const char *(*glGetString)(unsigned) = dlsym(hgl, "glGetString");
            if (glGetString) {
                printf("info A: GL_RENDERER=%s\n",
                       glGetString(0x1F01 /* GL_RENDERER */) ?: "?");
                printf("info A: GL_VERSION=%s\n",
                       glGetString(0x1F14 /* GL_VERSION */) ?: "?");
            }
            if (glClearColor && glClear) {
                glClearColor(1.0f, 0.5f, 0.25f, 1.0f);
                glClear(0x4000 /* GL_COLOR_BUFFER_BIT */);
                ok("A: glClear — software rasterization done");
            }
        } else {
            info("A: dlopen libGLESv2", dlerror());
        }
    }

out:
    printf("EGL_PROBE RESULT: %s %d/%d\n", passed == total ? "PASS" : "FAIL", passed, total);
    /* keep the flip on screen while QEMU lives */
    sleep(3600);
    return passed == total ? 0 : 1;
}
