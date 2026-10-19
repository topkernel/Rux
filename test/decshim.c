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
#include <sys/socket.h>
#include <sys/uio.h>
#include <time.h>
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
    if (probe_n < 400) {
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
            probe_n = 400;
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

/* ---- unix-socket / vsync-channel trace ----------------------------- *
 * cc11 showed exactly ONE eglSwapBuffers then a silent runner: the
 * animation starves after its first frame while the composer keeps
 * flipping RS repaints — the RS->client vsync channel (a unix
 * socketpair) is the prime suspect. These interposers record the
 * client side: which fds socketpair() creates and every small
 * recv/read on them (vsync ticks are 8-byte packets). */

static unsigned char unix_fds[4096]; /* fd -> 1 if from socketpair */
static int sock_trace_n = 0;

static unsigned long long mono_ms(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (unsigned long long)ts.tv_sec * 1000UL + ts.tv_nsec / 1000000UL;
}

typedef int (*socketpair_t)(int, int, int, int *);
static socketpair_t real_socketpair;

__attribute__((visibility("default")))
int decshim_socketpair(int domain, int type, int proto, int sv[2])
{
    if (!real_socketpair)
        real_socketpair = (socketpair_t)dlsym(RTLD_NEXT, "socketpair");
    int r = real_socketpair ? real_socketpair(domain, type, proto, sv) : -1;
    if (r == 0 && domain == 1 /* AF_UNIX */ && sock_trace_n < 400) {
        if (sv[0] >= 0 && sv[0] < 4096)
            unix_fds[sv[0]] = 1;
        if (sv[1] >= 0 && sv[1] < 4096)
            unix_fds[sv[1]] = 1;
        shim_log("DEC_SHIM[%llu]: socketpair type=%#x -> %d,%d",
                 mono_ms(), type, sv[0], sv[1]);
        sock_trace_n++;
    }
    return r;
}
asm(".globl socketpair\n.set socketpair, decshim_socketpair");

static void note_recv(const char *what, int fd, ssize_t r,
                      const void *buf, int fl)
{
    if (r <= 0 || r > 64)
        return;
    if (fd < 0 || fd >= 4096 || !unix_fds[fd])
        return;
    if (sock_trace_n >= 700)
        return;
    unsigned long long v = 0;
    memcpy(&v, buf, r < 8 ? (size_t)r : 8);
    shim_log("DEC_SHIM[%llu]: %s fd=%d n=%zd fl=%#x data=%016llx",
             mono_ms(), what, fd, r, fl, v);
    sock_trace_n++;
}

typedef ssize_t (*recv_t)(int, void *, size_t, int);
typedef ssize_t (*recvfrom_t)(int, void *, size_t, int,
                              void *, void *);
typedef ssize_t (*read_t)(int, void *, size_t);

__attribute__((visibility("default")))
ssize_t decshim_recv(int fd, void *buf, size_t n, int fl)
{
    static recv_t real;
    if (!real)
        real = (recv_t)dlsym(RTLD_NEXT, "recv");
    ssize_t r = real ? real(fd, buf, n, fl) : -1;
    note_recv("recv", fd, r, buf, fl);
    return r;
}
asm(".globl recv\n.set recv, decshim_recv");

__attribute__((visibility("default")))
ssize_t decshim_recvfrom(int fd, void *buf, size_t n, int fl,
                         void *addr, void *len)
{
    static recvfrom_t real;
    if (!real)
        real = (recvfrom_t)dlsym(RTLD_NEXT, "recvfrom");
    ssize_t r = real ? real(fd, buf, n, fl, addr, len) : -1;
    note_recv("recvfrom", fd, r, buf, fl);
    return r;
}
asm(".globl recvfrom\n.set recvfrom, decshim_recvfrom");

__attribute__((visibility("default")))
ssize_t decshim_read(int fd, void *buf, size_t n)
{
    static read_t real;
    if (!real)
        real = (read_t)dlsym(RTLD_NEXT, "read");
    ssize_t r = real ? real(fd, buf, n) : -1;
    note_recv("read", fd, r, buf, 0);
    return r;
}
asm(".globl read\n.set read, decshim_read");

typedef ssize_t (*send_t)(int, const void *, size_t, int);
typedef ssize_t (*write_t)(int, const void *, size_t);

__attribute__((visibility("default")))
ssize_t decshim_send(int fd, const void *buf, size_t n, int fl)
{
    static send_t real;
    if (!real)
        real = (send_t)dlsym(RTLD_NEXT, "send");
    ssize_t r = real ? real(fd, buf, n, fl) : -1;
    note_recv("send", fd, r, buf, fl);
    return r;
}
asm(".globl send\n.set send, decshim_send");

__attribute__((visibility("default")))
ssize_t decshim_sendto(int fd, const void *buf, size_t n, int fl,
                       const void *addr, void *len)
{
    static recvfrom_t real;
    if (!real)
        real = (recvfrom_t)dlsym(RTLD_NEXT, "sendto");
    ssize_t r = real ? real(fd, (void *)buf, n, fl, (void *)addr, len) : -1;
    note_recv("sendto", fd, r, buf, fl);
    return r;
}
asm(".globl sendto\n.set sendto, decshim_sendto");

__attribute__((visibility("default")))
ssize_t decshim_write(int fd, const void *buf, size_t n)
{
    static write_t real;
    if (!real)
        real = (write_t)dlsym(RTLD_NEXT, "write");
    ssize_t r = real ? real(fd, buf, n) : -1;
    note_recv("write", fd, r, buf, 0);
    return r;
}
asm(".globl write\n.set write, decshim_write");

/* ---- usleep / condvar timing (composer_host injection) ------------- *
 * The vendor DrmVsyncWorker paces display vsync with usleep(16000) +
 * a pthread condvar; standalone the kernel sleeps 16.2ms avg (sleep_
 * probe), but the real boot starves the whole pipeline to ~1 frame
 * per many seconds. These interposers time the sleeps and long
 * condvar waits INSIDE the real composer process. */

typedef int (*usleep_t)(unsigned);
typedef int (*cond_wait_t)(void *, void *);

__attribute__((visibility("default")))
int decshim_usleep(unsigned us)
{
    static usleep_t real;
    if (!real)
        real = (usleep_t)dlsym(RTLD_NEXT, "usleep");
    unsigned long long t0 = mono_ms();
    int r = real ? real(us) : -1;
    unsigned long long d = mono_ms() - t0;
    if (us >= 15000 && us <= 17000 && sock_trace_n < 900) {
        shim_log("DEC_SHIM[%llu]: usleep(%u) took %llu ms", mono_ms(), us, d);
        sock_trace_n++;
    }
    return r;
}
asm(".globl usleep\n.set usleep, decshim_usleep");

__attribute__((visibility("default")))
int decshim_pthread_cond_wait(void *cv, void *lk)
{
    static cond_wait_t real;
    if (!real)
        real = (cond_wait_t)dlsym(RTLD_NEXT, "pthread_cond_wait");
    unsigned long long t0 = mono_ms();
    int r = real ? real(cv, lk) : -1;
    unsigned long long d = mono_ms() - t0;
    if (d >= 100 && sock_trace_n < 950) {
        shim_log("DEC_SHIM[%llu]: cond_wait took %llu ms", mono_ms(), d);
        sock_trace_n++;
    }
    return r;
}
asm(".globl pthread_cond_wait\n.set pthread_cond_wait, decshim_pthread_cond_wait");

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
