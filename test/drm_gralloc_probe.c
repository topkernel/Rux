/* drm_gralloc_probe — freestanding DRM render-node test for Rux.
 *
 * Raw syscalls only (no libc), mirroring nettest.c. Exercises the exact
 * ioctl sequence the OpenHarmony buffer allocator issues against
 * /dev/dri/renderD128 (drivers/peripheral/display/buffer/vdi_base/src/
 * display_gralloc_gbm.cpp + hi_gbm.cpp):
 *
 *   open(O_RDWR|O_CLOEXEC) → DROP_MASTER → (VERSION/GET_CAP sanity) →
 *   CREATE_DUMB(720x1280x32) → PRIME_HANDLE_TOFD(DRM_CLOEXEC|DRM_RDWR) →
 *   mmap(prime_fd) → write/read pattern → DMA_BUF_IOCTL_SYNC →
 *   PRIME_FD_TO_HANDLE → munmap → DESTROY_DUMB
 *
 * plus MAP_DUMB mmap through the node fd and GEM_FLINK/GEM_OPEN.
 *
 * Ioctl encodings are LP64 (riscv64/x86_64) and match the OH libdrm
 * fork (third_party/libdrm/include/drm/drm.h); drm_prime_handle is 12
 * bytes. Prints "DRMPROBE PASS" / "DRMPROBE FAIL: <n>" and exits 0/1.
 */
typedef unsigned long u64;
typedef long s64;
typedef int s32;
typedef unsigned int u32;
typedef unsigned short u16;

#define __NR_write 64
#define __NR_exit 93
#define __NR_exit_group 94
#define __NR_ioctl 29
#define __NR_openat 56
#define __NR_close 57
#define __NR_mmap 222
#define __NR_munmap 215

#define O_RDONLY 0
#define O_RDWR 2
#define O_CLOEXEC 02000000

#define PROT_READ 1
#define PROT_WRITE 2
#define MAP_SHARED 1

/* DRM ioctls, LP64 encoding: _IOC(dir,size,nr) | 'd'<<8 */
#define DRM_IOCTL_VERSION          0xC0406400UL /* _IOWR(0x00,64) */
#define DRM_IOCTL_GET_CAP          0xC010640CUL /* _IOWR(0x0c,16) */
#define DRM_IOCTL_DROP_MASTER      0x0000641FUL /* _IO(0x1f) */
#define DRM_IOCTL_GEM_CLOSE        0x40086409UL /* _IOW(0x09,8) */
#define DRM_IOCTL_GEM_FLINK        0xC010640AUL /* _IOWR(0x0a,16) */
#define DRM_IOCTL_GEM_OPEN         0xC010640BUL /* _IOWR(0x0b,16) */
#define DRM_IOCTL_PRIME_HANDLE_TO_FD 0xC00C642DUL /* _IOWR(0x2d,12) */
#define DRM_IOCTL_PRIME_FD_TO_HANDLE 0xC00C642EUL /* _IOWR(0x2e,12) */
#define DRM_IOCTL_MODE_CREATE_DUMB 0xC02064B2UL /* _IOWR(0xb2,32) */
#define DRM_IOCTL_MODE_MAP_DUMB    0xC01064B3UL /* _IOWR(0xb3,16) */
#define DRM_IOCTL_MODE_DESTROY_DUMB 0xC00464B4UL /* _IOWR(0xb4,4) */
#define DMA_BUF_IOCTL_SYNC         0x40086200UL /* _IOW('b',0,8) */

#define DRM_CAP_DUMB_BUFFER 1
#define DRM_CAP_PRIME 5
#define DRM_CLOEXEC O_CLOEXEC
#define DRM_RDWR O_RDWR

struct create_dumb {
    u32 height, width, bpp, flags;
    u32 handle, pitch;
    u64 size;
};
struct map_dumb { u32 handle, pad; u64 offset; };
struct prime { u32 handle, flags; s32 fd; };
struct drm_version {
    s32 major, minor, patch;
    u64 name_len;  char *name;
    u64 date_len;  char *date;
    u64 desc_len;  char *desc;
};
struct get_cap { u64 capability, value; };
struct gem_flink { u32 handle, pad; u64 name; };
struct gem_open { u32 name, handle; u64 size; };

static s64 sys3(s64 n, s64 a, s64 b, s64 c)
{
    register s64 a0 asm("a0") = a;
    register s64 a1 asm("a1") = b;
    register s64 a2 asm("a2") = c;
    register s64 a7 asm("a7") = n;
    asm volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a7) : "memory");
    return a0;
}

/* gcc turns struct zero-init into memset even with -fno-builtin */
void *memset(void *d, int c, unsigned long n)
{
    unsigned char *p = d;
    while (n--) *p++ = (unsigned char)c;
    return d;
}
static s64 sys6(s64 n, s64 a, s64 b, s64 c, s64 d, s64 e, s64 f)
{
    register s64 a0 asm("a0") = a;
    register s64 a1 asm("a1") = b;
    register s64 a2 asm("a2") = c;
    register s64 a3 asm("a3") = d;
    register s64 a4 asm("a4") = e;
    register s64 a5 asm("a5") = f;
    register s64 a7 asm("a7") = n;
    asm volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a5), "r"(a7) : "memory");
    return a0;
}

static void wstr(const char *s)
{
    int n = 0;
    while (s[n]) n++;
    sys3(__NR_write, 1, (s64)s, n);
}
static void wnum(s64 v)
{
    char b[24]; int i = 23; b[i--] = 0;
    int neg = v < 0; if (neg) v = -v;
    do { b[i--] = '0' + (int)(v % 10); v /= 10; } while (v);
    if (neg) b[i--] = '-';
    sys3(__NR_write, 1, (s64)&b[i + 1], 23 - i - 1);
}
static void whex(u64 v)
{
    char b[19]; int i = 18; b[i--] = 0;
    do { int d = (int)(v & 0xf); b[i--] = d < 10 ? '0' + d : 'a' + d - 10; v >>= 4; } while (v);
    b[i--] = 'x'; b[i--] = '0';
    sys3(__NR_write, 1, (s64)&b[i + 1], 18 - i - 1);
}

static int fails;
static void check(const char *what, s64 got, s64 want)
{
    wstr("  "); wstr(what); wstr(": ");
    if (want == -999) { /* just print */ wnum(got); wstr("\n"); return; }
    if (got != want) {
        wstr("FAIL (got "); wnum(got); wstr(" want "); wnum(want); wstr(")\n");
        fails++;
    } else {
        wstr("ok ("); wnum(got); wstr(")\n");
    }
}

void _start(void)
{
    wstr("DRMPROBE start\n");

    /* 1. open renderD128 exactly like InitGbmDevice */
    s64 drmfd = sys3(__NR_openat, -100, (s64)"/dev/dri/renderD128", O_RDWR | O_CLOEXEC);
    if (drmfd < 0) { wstr("  open renderD128: FAIL "); wnum(drmfd); wstr("\n"); goto fail; }
    wstr("  open renderD128: ok (fd "); wnum(drmfd); wstr(")\n");

    /* 2. drmDropMaster (return ignored by the gralloc, but must be >= 0) */
    check("drop_master", sys3(__NR_ioctl, drmfd, DRM_IOCTL_DROP_MASTER, 0), 0);

    /* 3. VERSION: driver name must be NUL-terminated and present */
    {
        char name[32] = {0};
        struct drm_version v = {0};
        v.name_len = sizeof(name) - 1; v.name = name;
        s64 r = sys3(__NR_ioctl, drmfd, DRM_IOCTL_VERSION, (s64)&v);
        check("version ret", r, 0);
        wstr("  version name: "); wstr(name); wstr("\n");
        check("version name[0]", name[0] != 0, 1);
    }

    /* 4. GET_CAP */
    {
        struct get_cap c = { DRM_CAP_DUMB_BUFFER, 0 };
        check("cap dumb_buffer", sys3(__NR_ioctl, drmfd, DRM_IOCTL_GET_CAP, (s64)&c), 0);
        check("cap dumb_buffer val", (s64)c.value, 1);
        c.capability = DRM_CAP_PRIME; c.value = 0;
        sys3(__NR_ioctl, drmfd, DRM_IOCTL_GET_CAP, (s64)&c);
        check("cap prime val", (s64)c.value, 3);
    }

    /* 5. CREATE_DUMB — gralloc's RGBA_8888 boot buffer (hi_gbm table) */
    struct create_dumb db = { .height = 1280, .width = 720, .bpp = 32, .flags = 0 };
    check("create_dumb ret", sys3(__NR_ioctl, drmfd, DRM_IOCTL_MODE_CREATE_DUMB, (s64)&db), 0);
    wstr("  handle "); wnum(db.handle); wstr(" pitch "); wnum(db.pitch);
    wstr(" size "); wnum((s64)db.size); wstr("\n");
    if (!db.handle || !db.pitch || db.size < (u64)db.pitch * 1280) { wstr("  bad dumb geometry\n"); goto fail; }

    /* 6. PRIME_HANDLE_TO_FD with DRM_CLOEXEC|DRM_RDWR (HdiGbmBoGetFd) */
    struct prime ex = { db.handle, DRM_CLOEXEC | DRM_RDWR, -1 };
    check("prime_handle_to_fd ret", sys3(__NR_ioctl, drmfd, DRM_IOCTL_PRIME_HANDLE_TO_FD, (s64)&ex), 0);
    if (ex.fd < 0) { wstr("  prime fd invalid\n"); goto fail; }
    wstr("  prime fd: "); wnum(ex.fd); wstr("\n");

    /* 7. mmap the dma-buf (GbmMmap) and write a pattern */
    u64 len = db.size;
    u64 *m1 = (u64 *)sys6(__NR_mmap, 0, (s64)len, PROT_READ | PROT_WRITE, MAP_SHARED, ex.fd, 0);
    if ((s64)m1 < 0 && (s64)m1 > -4096) { wstr("  mmap prime fd: FAIL "); wnum((s64)m1); wstr("\n"); goto fail; }
    u64 *m2 = (u64 *)sys6(__NR_mmap, 0, (s64)len, PROT_READ | PROT_WRITE, MAP_SHARED, ex.fd, 0);
    if ((s64)m2 < 0 && (s64)m2 > -4096) { wstr("  mmap#2 prime fd: FAIL "); wnum((s64)m2); wstr("\n"); goto fail; }
    m1[0] = 0x1122334455667788UL; m1[1] = 0xaabbccdd00001122UL;
    check("map alias readback m2[0]", (s64)m2[0], 0x1122334455667788L);
    check("map alias readback m2[1]", (s64)m2[1], 0xaabbccdd00001122L);
    m1[len / 8 - 1] = 0x5555aaa15555aaa1UL; /* last byte of the buffer */
    check("map tail readback m2", (s64)m2[len / 8 - 1], 0x5555aaa15555aaa1L);

    /* 8. DMA_BUF_IOCTL_SYNC (gralloc's DmaBufferSync) */
    {
        u64 flags = 0x6; /* DMA_BUF_SYNC_RW | DMA_BUF_SYNC_START */
        check("dma_buf_sync", sys3(__NR_ioctl, ex.fd, DMA_BUF_IOCTL_SYNC, (s64)&flags), 0);
    }

    /* 9. PRIME_FD_TO_HANDLE (the composer's import path) */
    struct prime im = { 0, 0, ex.fd };
    check("prime_fd_to_handle ret", sys3(__NR_ioctl, drmfd, DRM_IOCTL_PRIME_FD_TO_HANDLE, (s64)&im), 0);
    check("import handle matches", (s64)im.handle, (s64)db.handle);

    /* 10. GEM_FLINK + GEM_CLOSE + GEM_OPEN (name survives handle close
     * because the prime fd holds the object). */
    struct gem_flink fl = { db.handle, 0, 0 };
    check("flink ret", sys3(__NR_ioctl, drmfd, DRM_IOCTL_GEM_FLINK, (s64)&fl), 0);
    {
        u32 h = db.handle;
        check("gem_close ret", sys3(__NR_ioctl, drmfd, DRM_IOCTL_GEM_CLOSE, (s64)&h), 0);
    }
    struct gem_open go = { (u32)fl.name, 0, 0 };
    check("gem_open ret", sys3(__NR_ioctl, drmfd, DRM_IOCTL_GEM_OPEN, (s64)&go), 0);
    check("gem_open handle", (s64)go.handle, (s64)db.handle);
    check("gem_open size", (s64)go.size, (s64)db.size);

    /* 11. MAP_DUMB path on the node fd (mesa gbm_mmap route) */
    struct map_dumb md = { go.handle, 0, 0 };
    check("map_dumb ret", sys3(__NR_ioctl, drmfd, DRM_IOCTL_MODE_MAP_DUMB, (s64)&md), 0);
    u64 *m3 = (u64 *)sys6(__NR_mmap, 0, (s64)len, PROT_READ | PROT_WRITE, MAP_SHARED, drmfd, (s64)md.offset);
    if ((s64)m3 < 0 && (s64)m3 > -4096) { wstr("  mmap node fd: FAIL "); wnum((s64)m3); wstr("\n"); goto fail; }
    check("node-fd map readback", (s64)m3[0], 0x1122334455667788L);
    sys3(__NR_munmap, (s64)m3, (s64)len, 0);

    /* 12. teardown: unmap, close prime fd, destroy dumb */
    sys3(__NR_munmap, (s64)m1, (s64)len, 0);
    sys3(__NR_munmap, (s64)m2, (s64)len, 0);
    check("close prime fd", sys3(__NR_close, ex.fd, 0, 0), 0);
    {
        u32 h = go.handle;
        check("destroy_dumb", sys3(__NR_ioctl, drmfd, DRM_IOCTL_MODE_DESTROY_DUMB, (s64)&h), 0);
    }
    check("destroy again EINVAL", sys3(__NR_ioctl, drmfd, DRM_IOCTL_MODE_DESTROY_DUMB, (s64)&(u32){ go.handle }), -22);
    check("close drm fd", sys3(__NR_close, drmfd, 0, 0), 0);

    if (fails) goto fail;
    wstr("DRMPROBE PASS\n");
    sys3(__NR_exit_group, 0, 0, 0);
fail:
    wstr("DRMPROBE FAIL\n");
    sys3(__NR_exit_group, 1, 0, 0);
}
