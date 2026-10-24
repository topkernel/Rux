/*
 * decode_probe.c — OH bootanimation decode oracle (compose-content).
 *
 * The stock bootanimation draws every frame as canvas->clear(black) +
 * DrawImage(MakeFromEncoded(jpg)) — on Rux the flipped GEMs carry ONLY
 * the clear color (nz=921600 clear=921600 paint=0 over 150 frames), so
 * the image side never paints. MakeFromEncoded's result is appended to
 * the frame vector with NO null check in the bootanimation binary, and
 * the zip read path's error strings never fire — a decode failure is
 * silent by construction. This probe isolates the question "does the
 * image's own lib2d_graphics + libskia_canvaskit decode jpgs on Rux?"
 * in a minimal boot (probe as init, staged OH libs, no OH services):
 *
 *   - /test/logo40.jpg   : animation frame 40 (logo visible on host)
 *   - /test/logo40.png   : same pixels as PNG (different codec path)
 *   - /test/corrupt.jpg  : truncated header (negative control)
 *
 * Verdict line on serial: DECODE_PROBE RESULT: jpg=<ok|fail/null/0x0>
 * png=<...> corrupt=<...> — then sleeps forever (driver screendumps /
 * kills QEMU).
 */
#include <dlfcn.h>
#include <fcntl.h>
#include <link.h>
#include <signal.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* libc++ shared_ptr<T> layout: {T* ptr; __shared_weak_count* ctrl}. */
typedef struct { void *ptr; void *ctrl; } sp_t;

/* OH::Rosen::Drawing C++ ABI (reverse-engineered from the image's
 * lib2d_graphics.z.so — see the fault analysis in the commit log):
 * the public wrappers are thin shells over an impl pointer at offset
 * +8; the "static" factories actually dispatch through that impl, so
 * the object must be DEFAULT-CONSTRUCTED (C1Ev) before the call and
 * the call takes rdi = &wrapper (member-style), NOT an sret slot:
 *
 *   Data d; d.BuildFromMalloc(bytes, len);   // impl gets the SkData
 *   Image img; img.MakeFromEncoded(sp);      // sp = shared_ptr<Data>
 *
 * shared_ptr layout (libc++): {T* ptr; __shared_weak_count* ctrl};
 * the callee bumps ctrl+8 — a static dummy with a fat refcount works. */
typedef void (*obj_ctor_t)(void *obj);
typedef void (*build_from_malloc_t)(void *d, const void *data, size_t len);
typedef void (*make_from_encoded_t)(void *img, const sp_t *data);
typedef const void *(*get_data_t)(const void *d);
typedef int (*get_width_t)(const void *img);
typedef int (*get_height_t)(const void *img);

static build_from_malloc_t build_from_malloc;
static make_from_encoded_t make_from_encoded;
static get_data_t get_data;
static get_width_t get_width;
static get_height_t get_height;
static obj_ctor_t data_ctor;
static obj_ctor_t image_ctor;

/* fake __shared_weak_count: never released by the probe */
static unsigned char dummy_ctrl[32];

static void *xmalloc(size_t n)
{
    void *p = malloc(n ? n : 1);
    if (!p) {
        printf("DECODE_PROBE: malloc(%zu) failed\r\n", n);
        exit(1);
    }
    return p;
}

static unsigned char *read_file(const char *path, size_t *lenp)
{
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return NULL;
    size_t cap = 1 << 20, len = 0;
    unsigned char *buf = xmalloc(cap);
    for (;;) {
        if (len == cap) {
            cap *= 2;
            unsigned char *nb = xmalloc(cap);
            memcpy(nb, buf, len);
            free(buf);
            buf = nb;
        }
        ssize_t r = read(fd, buf + len, cap - len);
        if (r < 0) {
            close(fd);
            free(buf);
            return NULL;
        }
        if (r == 0)
            break;
        len += (size_t)r;
    }
    close(fd);
    *lenp = len;
    return buf;
}

static void fault_handler(int sig, siginfo_t *info, void *ucontext)
{
    ucontext_t *uc = (ucontext_t *)ucontext;
    /* greg indices for x86_64 musl: RIP=16, RSP=15, RAX=13... use the
     * ucontext fields directly (musl layout matches glibc here). */
    printf("\r\nDECODE_PROBE FAULT: sig=%d si_addr=%p si_code=%d "
           "rip=%#llx rsp=%#llx\r\n",
           sig, info ? info->si_addr : NULL, info ? info->si_code : -1,
           (unsigned long long)uc->uc_mcontext.gregs[16],
           (unsigned long long)uc->uc_mcontext.gregs[15]);
    fflush(stdout);
    _exit(42);
}

static int phdr_cb(struct dl_phdr_info *info, size_t sz, void *data)
{
    (void)sz; (void)data;
    if (info->dlpi_name && info->dlpi_name[0])
        printf("  base=%#llx %s\r\n",
               (unsigned long long)info->dlpi_addr, info->dlpi_name);
    else
        printf("  base=%#llx <main>\r\n",
               (unsigned long long)info->dlpi_addr);
    fflush(stdout);
    return 0;
}

static void print_maps(void)
{
    printf("DECODE_PROBE: phdr map:\r\n");
    fflush(stdout);
    dl_iterate_phdr(phdr_cb, NULL);
}

static void install_handlers(void)
{
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = fault_handler;
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGSEGV, &sa, NULL);
    sigaction(SIGBUS, &sa, NULL);
    sigaction(SIGILL, &sa, NULL);
    sigaction(SIGFPE, &sa, NULL);
    sigaction(SIGABRT, &sa, NULL);
}

/* Decode one encoded blob; returns 1 ok / 0 fail. Prints details. */
static int try_decode(const char *tag, const unsigned char *bytes, size_t len)
{
    /* default-construct the wrappers, then member-style calls */
    unsigned char dataobj[64];
    unsigned char imgobj[64];
    memset(dataobj, 0, sizeof dataobj);
    memset(imgobj, 0, sizeof imgobj);
    data_ctor(dataobj);
    image_ctor(imgobj);

    printf("DECODE_PROBE: %s BuildFromMalloc...\r\n", tag);
    fflush(stdout);
    build_from_malloc(dataobj, bytes, len);
    const void *raw = get_data(dataobj);
    printf("DECODE_PROBE: %s BuildFromMalloc -> GetData=%p (want %p)\r\n",
           tag, raw, (const void *)bytes);
    fflush(stdout);
    if (!raw) {
        return 0;
    }
    sp_t sp = { dataobj, (void *)dummy_ctrl };
    printf("DECODE_PROBE: %s MakeFromEncoded...\r\n", tag);
    fflush(stdout);
    make_from_encoded(imgobj, &sp);
    void *impl = *(void **)(imgobj + 8);
    int w = get_width(imgobj);
    int h = get_height(imgobj);
    printf("DECODE_PROBE: %s MakeFromEncoded -> impl=%p w=%d h=%d\r\n",
           tag, impl, w, h);
    fflush(stdout);
    return impl != NULL && w > 0 && h > 0;
}

int main(void)
{
    install_handlers();
    printf("DECODE_PROBE: start (pid should be 1)\r\n");

    void *h = dlopen("lib2d_graphics.z.so", RTLD_NOW | RTLD_LOCAL);
    if (!h) {
        printf("DECODE_PROBE: dlopen lib2d_graphics failed: %s\r\n",
               dlerror() ? dlerror() : "?");
        /* retry lazy for the error string */
        h = dlopen("lib2d_graphics.z.so", RTLD_LAZY | RTLD_LOCAL);
        if (!h) {
            printf("DECODE_PROBE RESULT: dlopen fail\r\n");
            for (;;)
                pause();
        }
    }
    printf("DECODE_PROBE: lib2d_graphics loaded\r\n");

    build_from_malloc = (build_from_malloc_t)dlsym(
        h, "_ZN4OHOS5Rosen7Drawing4Data15BuildFromMallocEPKvm");
    make_from_encoded = (make_from_encoded_t)dlsym(
        h, "_ZN4OHOS5Rosen7Drawing5Image15MakeFromEncodedERKNSt3__h10shared_ptrINS1_4DataEEE");
    get_data = (get_data_t)dlsym(
        h, "_ZNK4OHOS5Rosen7Drawing4Data7GetDataEv");
    get_width = (get_width_t)dlsym(
        h, "_ZNK4OHOS5Rosen7Drawing5Image8GetWidthEv");
    get_height = (get_height_t)dlsym(
        h, "_ZNK4OHOS5Rosen7Drawing5Image9GetHeightEv");
    data_ctor = (obj_ctor_t)dlsym(
        h, "_ZN4OHOS5Rosen7Drawing4DataC1Ev");
    image_ctor = (obj_ctor_t)dlsym(
        h, "_ZN4OHOS5Rosen7Drawing5ImageC1Ev");
    if (!build_from_malloc || !make_from_encoded || !get_width || !get_height
        || !data_ctor || !image_ctor || !get_data) {
        printf("DECODE_PROBE RESULT: dlsym fail bfm=%p mfe=%p gw=%p gh=%p "
               "dc=%p ic=%p gd=%p\r\n",
               (void *)build_from_malloc, (void *)make_from_encoded,
               (void *)get_width, (void *)get_height,
               (void *)data_ctor, (void *)image_ctor, (void *)get_data);
        for (;;)
            pause();
    }
    /* fat refcount so nothing frees during the calls */
    *(long *)(dummy_ctrl + 8) = 0x10000;
    printf("DECODE_PROBE: symbols resolved\r\n");
    print_maps();

    size_t len = 0;
    unsigned char *jpg = read_file("/test/logo40.jpg", &len);
    int r_jpg = 0;
    if (jpg) {
        printf("DECODE_PROBE: logo40.jpg %zu bytes, first4=%02x%02x%02x%02x\r\n",
               len, jpg[0], jpg[1], jpg[2], jpg[3]);
        r_jpg = try_decode("jpg", jpg, len);
        free(jpg);
    } else {
        printf("DECODE_PROBE: /test/logo40.jpg unreadable\r\n");
    }

    int r_png = 0;
    unsigned char *png = read_file("/test/logo40.png", &len);
    if (png) {
        printf("DECODE_PROBE: logo40.png %zu bytes\r\n", len);
        r_png = try_decode("png", png, len);
        free(png);
    } else {
        printf("DECODE_PROBE: /test/logo40.png unreadable\r\n");
    }

    int r_bad = 0;
    unsigned char *bad = read_file("/test/corrupt.jpg", &len);
    if (bad) {
        r_bad = try_decode("corrupt", bad, len);
        free(bad);
    }
    /* corrupt must FAIL for the oracle to be meaningful */
    printf("DECODE_PROBE RESULT: jpg=%s png=%s corrupt=%s(should be fail)\r\n",
           r_jpg ? "ok" : "FAIL", r_png ? "ok" : "FAIL",
           r_bad ? "ok-BAD" : "fail");
    for (;;)
        pause();
}
