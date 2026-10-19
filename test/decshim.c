/*
 * decshim.c — LD_PRELOAD shim for the OH bootanimation decode/draw
 * oracle (compose-content).
 *
 * Loaded via the bootanimation service cfg's env LD_PRELOAD on a
 * private system-image copy. Three instruments, all logged to
 * /dev/kmsg (falls back to /data/local/tmp/decshim.log):
 *
 *   1. constructor     — proves the preload took (pid, comm)
 *   2. Image::MakeFromEncoded — wraps the real one, logs w/h after:
 *      decode-failure (w=0) vs decode-success (w=800) per frame file
 *   3. eglSwapBuffers  — glReadPixels()s a 4x4 patch at the logo
 *      center BEFORE the real swap: what did the GL surface actually
 *      contain? (draw-failure vs blit-failure discriminator)
 *
 * ABI notes (from lib2d_graphics.z.so disassembly): MakeFromEncoded
 * takes rdi = Image* wrapper whose impl lives at +8 (pre-constructed
 * by Image::Image()); returns the Image wrapper by the impl tailcall.
 * GetWidth/GetHeight are const member fns (rdi = Image*).
 */
#include <dlfcn.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

typedef int (*get_width_t)(const void *);

static int log_fd = -1;
static int use_kmsg = 0;

static void shim_log_init(void)
{
    log_fd = open("/dev/kmsg", O_WRONLY);
    if (log_fd >= 0) {
        use_kmsg = 1;
        return;
    }
    log_fd = open("/data/local/tmp/decshim.log",
                  O_WRONLY | O_CREAT | O_APPEND, 0666);
}

static void shim_log(const char *fmt, ...)
{
    char buf[512];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, sizeof buf - 2, fmt, ap);
    va_end(ap);
    if (n < 0)
        return;
    if (n > (int)sizeof buf - 3)
        n = sizeof buf - 3;
    buf[n] = '\r';
    buf[n + 1] = '\n';
    if (log_fd >= 0)
        write(log_fd, buf, n + 2);
    else if (use_kmsg)
        ;
}

/* ---- Image::MakeFromEncoded(const std::shared_ptr<Data>&) --------- */

typedef void (*make_from_encoded_t)(void *img, const void *sp);
typedef int (*get_wh_t)(const void *img);

static make_from_encoded_t real_mfe;
static get_wh_t real_get_width;
static get_wh_t real_get_height;

__attribute__((visibility("default")))
void decshim_make_from_encoded(void *img, const void *sp)
{
    if (!real_mfe) {
        real_mfe = (make_from_encoded_t)dlsym(RTLD_NEXT,
            "_ZN4OHOS5Rosen7Drawing5Image15MakeFromEncodedERKNSt3__h10shared_ptrINS1_4DataEEE");
        if (!real_mfe) {
            shim_log("DEC_SHIM: dlsym MakeFromEncoded FAILED");
            return;
        }
    }
    real_mfe(img, sp);
    if (!real_get_width)
        real_get_width = (get_wh_t)dlsym(RTLD_NEXT,
            "_ZNK4OHOS5Rosen7Drawing5Image8GetWidthEv");
    if (!real_get_height)
        real_get_height = (get_wh_t)dlsym(RTLD_NEXT,
            "_ZNK4OHOS5Rosen7Drawing5Image9GetHeightEv");
    void *impl = *(void **)((unsigned char *)img + 8);
    int w = real_get_width ? real_get_width(img) : -1;
    int h = real_get_height ? real_get_height(img) : -1;
    shim_log("DEC_SHIM: MakeFromEncoded impl=%p w=%d h=%d", impl, w, h);
}

/* mangled alias so the loader binds the bootanimation's relocation to
 * the shim (the name is the contract) */
asm(".globl _ZN4OHOS5Rosen7Drawing5Image15MakeFromEncodedERKNSt3__h10shared_ptrINS1_4DataEEE\n"
    ".set _ZN4OHOS5Rosen7Drawing5Image15MakeFromEncodedERKNSt3__h10shared_ptrINS1_4DataEEE,"
    "decshim_make_from_encoded");

/* ---- eglSwapBuffers + glReadPixels probe -------------------------- */

typedef void *(*egl_get_proc_t)(const void *, const char *);
typedef unsigned int (*egl_swap_t)(void *, void *);
typedef void (*gl_read_pixels_t)(int, int, int, int, unsigned int,
                                 unsigned int, void *);
typedef unsigned int (*gl_get_error_t)(void);

static egl_swap_t real_swap;

__attribute__((visibility("default")))
unsigned int decshim_eglSwapBuffers(void *dpy, void *surf)
{
    static int probe_n = 0;
    if (probe_n < 40) {
        gl_read_pixels_t rpx = (gl_read_pixels_t)dlsym(RTLD_NEXT, "glReadPixels");
        gl_get_error_t gerr = (gl_get_error_t)dlsym(RTLD_NEXT, "glGetError");
        if (rpx) {
            unsigned char px[16 * 4];
            memset(px, 0xAA, sizeof px);
            rpx(358, 638, 4, 4, 0x1908 /*GL_RGBA*/, 0x1401 /*UBYTE*/, px);
            unsigned int e1 = gerr ? gerr() : 0xDEAD;
            unsigned int nz = 0;
            for (unsigned i = 0; i < sizeof px; i++)
                if (px[i])
                    nz++;
            shim_log("DEC_SHIM: swap#%d pre-read nz=%u/%zu p0=%02x%02x%02x%02x "
                     "glerr=%u", probe_n, nz, sizeof px,
                     px[0], px[1], px[2], px[3], e1);
            probe_n++;
        } else {
            shim_log("DEC_SHIM: glReadPixels not resolvable");
            probe_n = 40;
        }
    }
    if (!real_swap)
        real_swap = (egl_swap_t)dlsym(RTLD_NEXT, "eglSwapBuffers");
    if (!real_swap)
        return 0;
    return real_swap(dpy, surf);
}

asm(".globl eglSwapBuffers\n"
    ".set eglSwapBuffers, decshim_eglSwapBuffers");

/* ---- constructor --------------------------------------------------- */

__attribute__((constructor))
static void decshim_ctor(void)
{
    shim_log_init();
    char comm[64] = {0};
    int fd = open("/proc/self/comm", O_RDONLY);
    if (fd >= 0) {
        read(fd, comm, sizeof comm - 1);
        close(fd);
    }
    shim_log("DEC_SHIM: loaded pid=%d comm=%s kmsg=%d", getpid(), comm,
             use_kmsg);
}
