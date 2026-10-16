/*
 * drm_probe.c — DRM/KMS readiness probe for the OpenHarmony display HDI.
 *
 * Walks the EXACT ioctl sequence the OH composer + gralloc issue on
 * /dev/dri/card0 (studied from the image's libdisplay_composer_vendor.z.so
 * and the device/soc/edu/virt HAL sources):
 *
 *   composer init:
 *     open(card0) -> GETRESOURCES (counts>0)
 *     SET_CLIENT_CAP(UNIVERSAL_PLANES)=1, SET_CLIENT_CAP(ATOMIC)=1
 *     SET_MASTER, then the image libdrm's drmIsMaster probe = a single
 *     AUTH_MAGIC ioctl with magic 0 that must SUCCEED
 *     GETCRTC / GETENCODER / GETCONNECTOR (connected, PREFERRED mode)
 *     GETPLANERESOURCES / GETPLANE
 *     OBJ_GETPROPERTIES + GETPROPERTY on crtc/connector/plane; requires
 *       crtc:     ACTIVE, MODE_ID, OUT_FENCE_PTR
 *       connector: DPMS, CRTC_ID
 *       plane:    FB_ID, IN_FENCE_FD, CRTC_ID, type (valid enum value)
 *
 *   gralloc allocate:
 *     CREATE_DUMB(w,h,bpp) -> PRIME_HANDLE_TO_FD(DRM_CLOEXEC|DRM_RDWR)
 *     -> DESTROY_DUMB; mmap(prime_fd) RW SHARED; paint a test pattern
 *
 *   composer import + flip:
 *     PRIME_FD_TO_HANDLE -> ADDFB2(XBGR8888 720x1280)
 *     CREATEPROPBLOB(mode) -> MODE_ATOMIC{ OUT_FENCE_PTR, plane FB_ID +
 *     CRTC_ID, crtc ACTIVE=1 + MODE_ID=blob, connector CRTC_ID } with
 *     DRM_MODE_ATOMIC_ALLOW_MODESET (blocking commit)
 *
 * Prints one ok/FAIL line per step and a final verdict line:
 *     DRM_PROBE RESULT: PASS <n>/<n>
 *
 * Build (static musl, toolchain/x86_64-rux-linux-musl):
 *   gcc -static -nostdlib -O2 -I toolchain/x86_64-rux-linux-musl/include \
 *       -o drm_probe drm_probe.c \
 *       toolchain/x86_64-rux-linux-musl/lib/crt1.o \
 *       toolchain/x86_64-rux-linux-musl/lib/libc.a -lgcc
 *
 * Runs as init (init=/test/drm_probe) on a tiny ext4 rootfs, q35 +
 * virtio-gpu-pci 720x1280 — the same machine config the OH boots use.
 */
#include <fcntl.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/syscall.h>

/* ---- raw console output (write(1)) ---- */
static int puts_(const char *s) { return write(1, s, strlen(s)); }
static void putu_(unsigned long v) {
    char b[24]; int i = 24;
    if (!v) { puts_("0"); return; }
    while (v && i) { b[--i] = '0' + (v % 10); v /= 10; }
    write(1, b + i, 24 - i);
}
static void puth_(unsigned long v) {
    char b[19] = "0x"; int i = 18;
    puts_("0x");
    if (!v) { puts_("0"); return; }
    while (v && i > 2) { int d = v & 15; b[--i] = d < 10 ? '0' + d : 'a' + d - 10; v >>= 4; }
    write(1, b + i, 18 - i);
}

static int passed, total;
static void ok(const char *what) { puts_("ok   "); puts_(what); puts_("\n"); passed++; total++; }
static void fail(const char *what, long detail) {
    puts_("FAIL "); puts_(what); puts_(" errno="); putu_((unsigned long)(-detail));
    puts_(" ("); puts_(strerror(-detail)); puts_(")\n"); total++;
}
static void failm(const char *what, const char *msg) {
    puts_("FAIL "); puts_(what); puts_(" — "); puts_(msg); puts_("\n"); total++;
}

/* ---- DRM uapi (hand-rolled, no drm headers needed) ---- */
#define DRM_IOCTL_BASE 'd'
#define DRM_IOWR(nr, size)  ((3U<<30) | ((size)<<16) | (DRM_IOCTL_BASE<<8) | (nr))
#define DRM_IOW(nr, size)   ((1U<<30) | ((size)<<16) | (DRM_IOCTL_BASE<<8) | (nr))
#define DRM_IOR(nr, size)   ((2U<<30) | ((size)<<16) | (DRM_IOCTL_BASE<<8) | (nr))

struct drm_version { int major, minor, patch; int64_t name_len; char *name;
                     int64_t date_len; char *date; int64_t desc_len; char *desc; };
struct drm_set_client_cap { uint64_t capability, value; };
struct drm_mode_card_res { uint64_t fb_id_ptr, crtc_id_ptr, connector_id_ptr, encoder_id_ptr;
                           uint32_t count_fbs, count_crtcs, count_connectors, count_encoders;
                           uint32_t min_width, max_width, min_height, max_height; };
struct drm_mode_modeinfo { uint32_t clock; uint16_t hdisplay, hsync_start, hsync_end, htotal, hskew;
                           uint16_t vdisplay, vsync_start, vsync_end, vtotal, vscan;
                           uint32_t vrefresh, flags, type; char name[32]; };
/* kernel uapi layout: set_connectors_ptr, count_connectors, crtc_id,
 * fb_id, x, y, gamma_size, mode_valid, mode — 104 bytes total */
struct drm_mode_crtc { uint64_t set_connectors_ptr; uint32_t count_connectors;
                       uint32_t crtc_id, fb_id, x, y, gamma_size, mode_valid;
                       struct drm_mode_modeinfo mode; };
struct drm_mode_get_connector { uint64_t encoders_ptr, modes_ptr, props_ptr, prop_values_ptr;
                                uint32_t count_modes, count_props, count_encoders;
                                uint32_t encoder_id, connector_id, connector_type,
                                         connector_type_id, connection, mm_width, mm_height,
                                         subpixel, pad; };
struct drm_mode_get_encoder { uint32_t encoder_id, encoder_type, crtc_id;
                              uint32_t possible_crtcs, possible_clones; };
struct drm_mode_get_plane_res { uint64_t plane_id_ptr; uint32_t count_planes; };
struct drm_mode_get_plane { uint32_t plane_id, crtc_id, fb_id, possible_crtcs, gamma_size;
                            uint32_t count_formats; uint64_t format_ptr; };
struct drm_mode_obj_get_properties { uint64_t props_ptr, prop_values_ptr;
                                     uint32_t count_props, obj_id, obj_type, pad; };
struct drm_mode_get_property { uint64_t values_ptr, enum_blob_ptr;
                               uint32_t prop_id, flags; char name[32];
                               uint32_t count_values, count_enum_blobs; };
struct drm_mode_create_dumb { uint32_t height, width, bpp, flags;
                              uint32_t handle, pitch; uint64_t size; };
struct drm_mode_destroy_dumb { uint32_t handle; };
struct drm_prime_handle { int32_t handle; uint32_t flags; int32_t fd; uint32_t pad; };
struct drm_mode_fb_cmd2 { uint32_t fb_id, width, height, pixel_format, flags;
                          uint32_t handles[4], pitches[4], offsets[4]; uint64_t modifier[4]; };
struct drm_mode_create_blob { uint64_t data; uint32_t length; uint32_t blob_id; };
struct drm_mode_destroy_blob { uint32_t blob_id; };
struct drm_atomic { uint32_t flags, count_objs;
                    uint64_t objs_ptr, count_props_ptr, props_ptr, prop_values_ptr,
                            reserved, user_data; };
struct drm_mode_map_dumb { uint32_t handle, pad; uint64_t offset; };

#define DRM_IOCTL_VERSION                 DRM_IOWR(0x00, sizeof(struct drm_version))
#define DRM_IOCTL_SET_CLIENT_CAP          DRM_IOW(0x0d, sizeof(struct drm_set_client_cap))
#define DRM_IO(nr)                       ((0U<<30) | (DRM_IOCTL_BASE<<8) | (nr))
#define DRM_IOCTL_SET_MASTER              DRM_IO(0x1e)
#define DRM_IOCTL_DROP_MASTER             DRM_IO(0x1f)
#define DRM_IOCTL_AUTH_MAGIC              DRM_IOW(0x11, 4)
#define DRM_IOCTL_GEM_CLOSE               DRM_IOW(0x09, 4)
#define DRM_IOCTL_MODE_GETRESOURCES       DRM_IOWR(0xa0, sizeof(struct drm_mode_card_res))
#define DRM_IOCTL_MODE_GETCRTC            DRM_IOWR(0xa1, sizeof(struct drm_mode_crtc))
#define DRM_IOCTL_MODE_GETENCODER         DRM_IOWR(0xa6, sizeof(struct drm_mode_get_encoder))
#define DRM_IOCTL_MODE_GETCONNECTOR       DRM_IOWR(0xa7, sizeof(struct drm_mode_get_connector))
#define DRM_IOCTL_MODE_GETPLANERESOURCES  DRM_IOWR(0xb5, sizeof(struct drm_mode_get_plane_res))
#define DRM_IOCTL_MODE_GETPLANE           DRM_IOWR(0xb6, sizeof(struct drm_mode_get_plane))
#define DRM_IOCTL_MODE_OBJ_GETPROPERTIES  DRM_IOWR(0xb9, sizeof(struct drm_mode_obj_get_properties))
#define DRM_IOCTL_MODE_GETPROPERTY        DRM_IOWR(0xaa, sizeof(struct drm_mode_get_property))
#define DRM_IOCTL_MODE_CREATE_DUMB        DRM_IOWR(0xb2, sizeof(struct drm_mode_create_dumb))
#define DRM_IOCTL_MODE_DESTROY_DUMB       DRM_IOWR(0xb4, sizeof(struct drm_mode_destroy_dumb))
#define DRM_IOCTL_MODE_MAP_DUMB           DRM_IOWR(0xb3, sizeof(struct drm_mode_map_dumb))
#define DRM_IOCTL_PRIME_HANDLE_TO_FD      DRM_IOWR(0x2d, sizeof(struct drm_prime_handle))
#define DRM_IOCTL_PRIME_FD_TO_HANDLE      DRM_IOWR(0x2e, sizeof(struct drm_prime_handle))
#define DRM_IOCTL_MODE_ADDFB2             DRM_IOWR(0xb8, sizeof(struct drm_mode_fb_cmd2))
#define DRM_IOCTL_MODE_RMFB               DRM_IOWR(0xaf, 4)
#define DRM_IOCTL_MODE_CREATEPROPBLOB     DRM_IOWR(0xbd, sizeof(struct drm_mode_create_blob))
#define DRM_IOCTL_MODE_DESTROYPROPBLOB    DRM_IOWR(0xbe, 4)
#define DRM_IOCTL_MODE_ATOMIC             DRM_IOWR(0xbc, sizeof(struct drm_atomic))

#define DRM_CLIENT_CAP_UNIVERSAL_PLANES 2
#define DRM_CLIENT_CAP_ATOMIC 3
#define DRM_MODE_OBJECT_CRTC 0xcccccccc
#define DRM_MODE_OBJECT_CONNECTOR 0xc0c0c0c0
#define DRM_MODE_OBJECT_PLANE 0xeeeeeeee
#define DRM_MODE_CONNECTED 1
#define DRM_MODE_TYPE_PREFERRED (1<<3)
#define DRM_FORMAT_XBGR8888 0x34324258u
#define DRM_MODE_ATOMIC_ALLOW_MODESET (1<<3)

/* required property names per object (OH DrmCrtc/DrmConnector/DrmPlane Init) */
static const char *const crtc_need[] = { "ACTIVE", "MODE_ID", "OUT_FENCE_PTR", 0 };
static const char *const conn_need[] = { "DPMS", "CRTC_ID", 0 };
static const char *const plane_need[] = { "FB_ID", "IN_FENCE_FD", "CRTC_ID", "type", 0 };

/* count props on an object, find required ones by name */
static int check_props(int fd, uint32_t obj_id, uint32_t obj_type,
                       const char *const need[], const char *tag)
{
    struct drm_mode_obj_get_properties gp;
    memset(&gp, 0, sizeof gp);
    gp.obj_id = obj_id; gp.obj_type = obj_type;
    if (ioctl(fd, DRM_IOCTL_MODE_OBJ_GETPROPERTIES, &gp) < 0) {
        fail("OBJ_GETPROPERTIES", errno); return -1;
    }
    uint32_t n = gp.count_props;
    if (n == 0 || n > 64) { failm(tag, "no properties on object"); return -1; }
    uint32_t ids[64]; uint64_t vals[64];
    gp.props_ptr = (uintptr_t)ids; gp.prop_values_ptr = (uintptr_t)vals;
    gp.count_props = n;
    if (ioctl(fd, DRM_IOCTL_MODE_OBJ_GETPROPERTIES, &gp) < 0) {
        fail("OBJ_GETPROPERTIES#2", errno); return -1;
    }
    int missing = 0;
    for (const char *const *w = need; *w; w++) {
        int found = 0;
        for (uint32_t i = 0; i < n && !found; i++) {
            struct drm_mode_get_property p;
            memset(&p, 0, sizeof p);
            p.prop_id = ids[i];
            if (ioctl(fd, DRM_IOCTL_MODE_GETPROPERTY, &p) < 0) continue;
            if (!strncmp(p.name, *w, sizeof p.name)) {
                found = 1;
                /* 'type' must carry a valid enum value (Primary/Overlay/Cursor) */
                if (!strcmp(*w, "type") && vals[i] > 2) {
                    puts_("FAIL type enum value "); putu_((unsigned long)vals[i]); puts_("\n");
                    total++; missing++;
                }
            }
        }
        if (!found) {
            puts_("FAIL missing prop '"); puts_(*w); puts_("' on "); puts_(tag); puts_("\n");
            total++; missing++;
        }
    }
    if (!missing) { ok(tag); }
    return missing ? -1 : 0;
}

int main(void)
{
    puts_("drm_probe: start\n");

    /* ---- 1. open + version ---- */
    int fd = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
    if (fd < 0) { fail("open card0", errno); goto out; }
    ok("open /dev/dri/card0");

    struct drm_version ver;
    char vname[32] = {0};
    memset(&ver, 0, sizeof ver);
    ver.name = vname; ver.name_len = sizeof vname;
    if (ioctl(fd, DRM_IOCTL_VERSION, &ver) < 0) fail("VERSION", errno);
    else { puts_("ok   VERSION name="); puts_(vname); puts_("\n"); passed++; total++; }

    /* ---- 2. resources (counts) ---- */
    struct drm_mode_card_res res;
    memset(&res, 0, sizeof res);
    if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res) < 0) {
        fail("GETRESOURCES", errno); goto out;
    }
    if (res.count_crtcs > 0 && res.count_connectors > 0 && res.count_encoders > 0) {
        puts_("ok   GETRESOURCES crtcs="); putu_(res.count_crtcs);
        puts_(" conns="); putu_(res.count_connectors);
        puts_(" encs="); putu_(res.count_encoders); puts_("\n");
        passed++; total++;
    } else { failm("GETRESOURCES", "zero crtcs/connectors/encoders"); goto out; }

    uint32_t crtcs[8] = {0}, conns[8] = {0}, encs[8] = {0};
    res.crtc_id_ptr = (uintptr_t)crtcs; res.connector_id_ptr = (uintptr_t)conns;
    res.encoder_id_ptr = (uintptr_t)encs;
    res.count_crtcs = res.count_connectors = res.count_encoders = 8;
    if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res) < 0) {
        fail("GETRESOURCES#2", errno); goto out;
    }
    ok("GETRESOURCES ids");

    /* ---- 3. client caps (composer hard-fails without these) ---- */
    struct drm_set_client_cap cap;
    cap.capability = DRM_CLIENT_CAP_UNIVERSAL_PLANES; cap.value = 1;
    if (ioctl(fd, DRM_IOCTL_SET_CLIENT_CAP, &cap) < 0) fail("SET_CLIENT_CAP UNIVERSAL_PLANES", errno);
    else ok("SET_CLIENT_CAP UNIVERSAL_PLANES");
    cap.capability = DRM_CLIENT_CAP_ATOMIC; cap.value = 1;
    if (ioctl(fd, DRM_IOCTL_SET_CLIENT_CAP, &cap) < 0) fail("SET_CLIENT_CAP ATOMIC", errno);
    else ok("SET_CLIENT_CAP ATOMIC");

    /* ---- 4. master (the image libdrm's drmIsMaster = AUTH_MAGIC(0) success) ---- */
    if (ioctl(fd, DRM_IOCTL_SET_MASTER, 0) < 0) fail("SET_MASTER", errno);
    else ok("SET_MASTER");
    uint32_t magic0 = 0;
    if (ioctl(fd, DRM_IOCTL_AUTH_MAGIC, &magic0) < 0)
        fail("AUTH_MAGIC(0) [drmIsMaster probe]", errno);
    else ok("AUTH_MAGIC(0) [drmIsMaster probe]");

    /* ---- 5. crtc / encoder / connector / plane ---- */
    struct drm_mode_crtc crtc;
    memset(&crtc, 0, sizeof crtc);
    crtc.crtc_id = crtcs[0];
    if (ioctl(fd, DRM_IOCTL_MODE_GETCRTC, &crtc) < 0) fail("GETCRTC", errno);
    else ok("GETCRTC");

    struct drm_mode_get_encoder enc;
    memset(&enc, 0, sizeof enc);
    enc.encoder_id = encs[0];
    if (ioctl(fd, DRM_IOCTL_MODE_GETENCODER, &enc) < 0) fail("GETENCODER", errno);
    else if (enc.possible_crtcs == 0) failm("GETENCODER", "possible_crtcs empty");
    else ok("GETENCODER (possible_crtcs!=0)");

    struct drm_mode_get_connector conn;
    struct drm_mode_modeinfo modes[4];
    uint32_t conn_encs[4];
    memset(&conn, 0, sizeof conn);
    conn.connector_id = conns[0];
    conn.modes_ptr = (uintptr_t)modes; conn.encoders_ptr = (uintptr_t)conn_encs;
    conn.count_modes = 4; conn.count_encoders = 4;
    if (ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, &conn) < 0) {
        fail("GETCONNECTOR", errno); goto out;
    }
    if (conn.connection != DRM_MODE_CONNECTED || conn.count_modes < 1) {
        failm("GETCONNECTOR", "not connected / no modes"); goto out;
    }
    if (!(modes[0].type & DRM_MODE_TYPE_PREFERRED)) {
        failm("GETCONNECTOR", "mode lacks PREFERRED flag");
    } else {
        puts_("ok   GETCONNECTOR "); putu_(modes[0].hdisplay); puts_("x");
        putu_(modes[0].vdisplay); puts_(" preferred\n"); passed++; total++;
    }

    struct drm_mode_get_plane_res pres;
    memset(&pres, 0, sizeof pres);
    if (ioctl(fd, DRM_IOCTL_MODE_GETPLANERESOURCES, &pres) < 0) {
        fail("GETPLANERESOURCES", errno); goto out;
    }
    if (pres.count_planes < 1) { failm("GETPLANERESOURCES", "no planes"); goto out; }
    uint32_t planes[4];
    pres.plane_id_ptr = (uintptr_t)planes; pres.count_planes = 4;
    if (ioctl(fd, DRM_IOCTL_MODE_GETPLANERESOURCES, &pres) < 0) {
        fail("GETPLANERESOURCES#2", errno); goto out;
    }
    ok("GETPLANERESOURCES");

    struct drm_mode_get_plane plane;
    uint32_t fmts[16];
    memset(&plane, 0, sizeof plane);
    plane.plane_id = planes[0];
    plane.format_ptr = (uintptr_t)fmts;
    plane.count_formats = 16;
    if (ioctl(fd, DRM_IOCTL_MODE_GETPLANE, &plane) < 0) {
        fail("GETPLANE", errno); goto out;
    }
    {
        int has_xbgr = 0;
        for (uint32_t i = 0; i < plane.count_formats; i++)
            if (fmts[i] == DRM_FORMAT_XBGR8888) has_xbgr = 1;
        if (has_xbgr) ok("GETPLANE advertises XBGR8888");
        else failm("GETPLANE", "XBGR8888 not advertised (gralloc FB format)");
    }

    /* ---- 6. required properties ---- */
    if (check_props(fd, crtcs[0], DRM_MODE_OBJECT_CRTC, crtc_need, "crtc props") < 0) goto out;
    if (check_props(fd, conns[0], DRM_MODE_OBJECT_CONNECTOR, conn_need, "connector props") < 0) goto out;
    if (check_props(fd, planes[0], DRM_MODE_OBJECT_PLANE, plane_need, "plane props") < 0) goto out;

    /* re-discover prop IDs for the atomic commit */
    uint32_t crtc_active_id = 0, crtc_mode_id = 0, crtc_outfence_id = 0;
    uint32_t conn_crtc_id_id = 0, plane_fb_id = 0, plane_crtc_id_id = 0, plane_infence_id = 0;
    {
        struct drm_mode_obj_get_properties gp;
        memset(&gp, 0, sizeof gp);
        gp.obj_id = crtcs[0]; gp.obj_type = DRM_MODE_OBJECT_CRTC;
        ioctl(fd, DRM_IOCTL_MODE_OBJ_GETPROPERTIES, &gp);
        uint32_t ids[32]; uint64_t vals[32];
        gp.props_ptr = (uintptr_t)ids; gp.prop_values_ptr = (uintptr_t)vals;
        gp.count_props = gp.count_props > 32 ? 32 : gp.count_props;
        ioctl(fd, DRM_IOCTL_MODE_OBJ_GETPROPERTIES, &gp);
        for (uint32_t i = 0; i < gp.count_props; i++) {
            struct drm_mode_get_property p;
            memset(&p, 0, sizeof p); p.prop_id = ids[i];
            if (ioctl(fd, DRM_IOCTL_MODE_GETPROPERTY, &p) < 0) continue;
            if (!strcmp(p.name, "ACTIVE")) crtc_active_id = ids[i];
            if (!strcmp(p.name, "MODE_ID")) crtc_mode_id = ids[i];
            if (!strcmp(p.name, "OUT_FENCE_PTR")) crtc_outfence_id = ids[i];
        }
        memset(&gp, 0, sizeof gp);
        gp.obj_id = conns[0]; gp.obj_type = DRM_MODE_OBJECT_CONNECTOR;
        ioctl(fd, DRM_IOCTL_MODE_OBJ_GETPROPERTIES, &gp);
        gp.props_ptr = (uintptr_t)ids; gp.prop_values_ptr = (uintptr_t)vals;
        gp.count_props = gp.count_props > 32 ? 32 : gp.count_props;
        ioctl(fd, DRM_IOCTL_MODE_OBJ_GETPROPERTIES, &gp);
        for (uint32_t i = 0; i < gp.count_props; i++) {
            struct drm_mode_get_property p;
            memset(&p, 0, sizeof p); p.prop_id = ids[i];
            if (ioctl(fd, DRM_IOCTL_MODE_GETPROPERTY, &p) < 0) continue;
            if (!strcmp(p.name, "CRTC_ID")) conn_crtc_id_id = ids[i];
        }
        memset(&gp, 0, sizeof gp);
        gp.obj_id = planes[0]; gp.obj_type = DRM_MODE_OBJECT_PLANE;
        ioctl(fd, DRM_IOCTL_MODE_OBJ_GETPROPERTIES, &gp);
        gp.props_ptr = (uintptr_t)ids; gp.prop_values_ptr = (uintptr_t)vals;
        gp.count_props = gp.count_props > 32 ? 32 : gp.count_props;
        ioctl(fd, DRM_IOCTL_MODE_OBJ_GETPROPERTIES, &gp);
        for (uint32_t i = 0; i < gp.count_props; i++) {
            struct drm_mode_get_property p;
            memset(&p, 0, sizeof p); p.prop_id = ids[i];
            if (ioctl(fd, DRM_IOCTL_MODE_GETPROPERTY, &p) < 0) continue;
            if (!strcmp(p.name, "FB_ID")) plane_fb_id = ids[i];
            if (!strcmp(p.name, "CRTC_ID")) plane_crtc_id_id = ids[i];
            if (!strcmp(p.name, "IN_FENCE_FD")) plane_infence_id = ids[i];
        }
    }
    if (crtc_active_id && crtc_mode_id && crtc_outfence_id && conn_crtc_id_id &&
        plane_fb_id && plane_crtc_id_id && plane_infence_id)
        ok("prop id re-discovery");
    else { failm("prop id re-discovery", "missing ids"); goto out; }

    /* ---- 7. gralloc path: dumb buffer + prime export + mmap + paint ---- */
    struct drm_mode_create_dumb dumb;
    memset(&dumb, 0, sizeof dumb);
    dumb.width = 720; dumb.height = 1280; dumb.bpp = 32;
    if (ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &dumb) < 0) {
        fail("CREATE_DUMB 720x1280x32", errno); goto out;
    }
    if (dumb.pitch < 720 * 4 || dumb.size < (uint64_t)720 * 4 * 1280) {
        puts_("FAIL CREATE_DUMB geometry pitch="); putu_(dumb.pitch);
        puts_(" size="); putu_((unsigned long)dumb.size); puts_("\n"); total++;
        goto out;
    }
    puts_("ok   CREATE_DUMB handle="); putu_(dumb.handle);
    puts_(" pitch="); putu_(dumb.pitch); puts_("\n"); passed++; total++;

    /* prime export: drm_prime_handle { handle, flags, fd, pad } */
    {
        struct drm_prime_handle exp;
        memset(&exp, 0, sizeof exp);
        exp.handle = (int32_t)dumb.handle;
        exp.flags = 0x80000u /*DRM_CLOEXEC*/ | 2u /*DRM_RDWR*/;
        if (ioctl(fd, DRM_IOCTL_PRIME_HANDLE_TO_FD, &exp) < 0) {
            fail("PRIME_HANDLE_TO_FD", errno); goto out;
        }
        int pfd = exp.fd;
        if (pfd < 0) { failm("PRIME_HANDLE_TO_FD", "negative fd"); goto out; }
        ok("PRIME_HANDLE_TO_FD");

        /* destroy the dumb handle like the gralloc does (fd owns the object) */
        struct drm_mode_destroy_dumb dd = { .handle = dumb.handle };
        if (ioctl(fd, DRM_IOCTL_MODE_DESTROY_DUMB, &dd) < 0)
            fail("DESTROY_DUMB after export (non-fatal)", errno);
        else ok("DESTROY_DUMB after export");

        /* mmap the prime fd and paint a pattern */
        void *map = mmap(NULL, dumb.size, PROT_READ | PROT_WRITE, MAP_SHARED, pfd, 0);
        if (map == MAP_FAILED) { fail("mmap(prime_fd)", errno); goto out; }
        ok("mmap(prime_fd) RW SHARED");
        uint32_t *px = (uint32_t *)map;
        for (uint32_t y = 0; y < 1280; y++)
            for (uint32_t x = 0; x < 720; x++) {
                uint32_t r = x * 255 / 719, g = y * 255 / 1279, b = 128;
                px[y * (dumb.pitch / 4) + x] = (r << 24) | (g << 16) | (b << 8);
            }

        /* re-import on this fd (composer does it on ITS card0 fd) */
        struct drm_prime_handle imp;
        memset(&imp, 0, sizeof imp);
        imp.fd = pfd;
        if (ioctl(fd, DRM_IOCTL_PRIME_FD_TO_HANDLE, &imp) < 0) {
            fail("PRIME_FD_TO_HANDLE", errno); goto out;
        }
        if (imp.handle == 0) { failm("PRIME_FD_TO_HANDLE", "handle 0"); goto out; }
        ok("PRIME_FD_TO_HANDLE");

        /* ---- 8. ADDFB2 with the gralloc FB format ---- */
        struct drm_mode_fb_cmd2 f2;
        memset(&f2, 0, sizeof f2);
        f2.width = 720; f2.height = 1280;
        f2.pixel_format = DRM_FORMAT_XBGR8888;
        f2.handles[0] = (uint32_t)imp.handle;
        f2.pitches[0] = dumb.pitch;
        if (ioctl(fd, DRM_IOCTL_MODE_ADDFB2, &f2) < 0) {
            fail("ADDFB2 XBGR8888", errno); goto out;
        }
        if (f2.fb_id == 0) { failm("ADDFB2", "fb_id 0"); goto out; }
        puts_("ok   ADDFB2 fb="); putu_(f2.fb_id); puts_("\n"); passed++; total++;

        /* ---- 9. mode blob ---- */
        struct drm_mode_create_blob cb;
        memset(&cb, 0, sizeof cb);
        cb.data = (uintptr_t)&modes[0];
        cb.length = sizeof(struct drm_mode_modeinfo);
        if (ioctl(fd, DRM_IOCTL_MODE_CREATEPROPBLOB, &cb) < 0 || cb.blob_id == 0) {
            fail("CREATEPROPBLOB(mode)", errno); goto out;
        }
        ok("CREATEPROPBLOB(mode)");

        /* ---- 10. blocking atomic commit with ALLOW_MODESET ---- */
        uint64_t out_fence = (uint64_t)-1;
        uint32_t objs[3] = { crtcs[0], planes[0], conns[0] };
        uint32_t counts[3] = { 3, 3, 1 };
        uint32_t props[7] = {
            crtc_outfence_id, crtc_active_id, crtc_mode_id,
            plane_fb_id, plane_crtc_id_id, plane_infence_id,
            conn_crtc_id_id,
        };
        uint64_t values[7];
        values[0] = (uintptr_t)&out_fence;
        values[1] = 1;                 /* ACTIVE */
        values[2] = cb.blob_id;        /* MODE_ID */
        values[3] = f2.fb_id;          /* plane FB_ID */
        values[4] = crtcs[0];          /* plane CRTC_ID */
        values[5] = (uint64_t)-1;      /* plane IN_FENCE_FD = -1 (no fence) */
        values[6] = crtcs[0];          /* connector CRTC_ID */
        struct drm_atomic a;
        memset(&a, 0, sizeof a);
        a.flags = DRM_MODE_ATOMIC_ALLOW_MODESET;
        a.count_objs = 3;
        a.objs_ptr = (uintptr_t)objs;
        a.count_props_ptr = (uintptr_t)counts;
        a.props_ptr = (uintptr_t)props;
        a.prop_values_ptr = (uintptr_t)values;
        long r = ioctl(fd, DRM_IOCTL_MODE_ATOMIC, &a);
        if (r < 0) { fail("MODE_ATOMIC commit", errno); goto out; }
        ok("MODE_ATOMIC commit (ALLOW_MODESET, blocking)");

        /* second flip: FB only, no modeset — the steady-state frame path */
        a.count_objs = 1;
        objs[0] = planes[0];
        counts[0] = 2;
        props[0] = plane_fb_id; values[0] = f2.fb_id;
        props[1] = plane_crtc_id_id; values[1] = crtcs[0];
        r = ioctl(fd, DRM_IOCTL_MODE_ATOMIC, &a);
        if (r < 0) fail("MODE_ATOMIC flip", errno);
        else ok("MODE_ATOMIC flip (no modeset)");

        /* teardown like the composer does */
        uint32_t fb = f2.fb_id;
        if (ioctl(fd, DRM_IOCTL_MODE_RMFB, &fb) < 0) fail("RMFB (non-fatal)", errno);
        else ok("RMFB");
        uint32_t gh = (uint32_t)imp.handle;
        if (ioctl(fd, DRM_IOCTL_GEM_CLOSE, &gh) < 0) fail("GEM_CLOSE (non-fatal)", errno);
        else ok("GEM_CLOSE");
        struct drm_mode_destroy_blob db = { .blob_id = cb.blob_id };
        if (ioctl(fd, DRM_IOCTL_MODE_DESTROYPROPBLOB, &db) < 0)
            fail("DESTROYPROPBLOB (non-fatal)", errno);
        else ok("DESTROYPROPBLOB");

        /* read back a pixel through a SECOND mapping to prove coherence */
        void *map2 = mmap(NULL, dumb.size, PROT_READ, MAP_SHARED, pfd, 0);
        if (map2 == MAP_FAILED) fail("mmap#2 coherence (non-fatal)", errno);
        else {
            uint32_t probe = ((uint32_t *)map2)[640 * (dumb.pitch / 4) + 360];
            if (((probe >> 24) & 0xff) != (360 * 255 / 719) || ((probe >> 16) & 0xff) != (640 * 255 / 1279))
                failm("prime mmap coherence", "second mapping sees stale data");
            else ok("prime mmap coherence");
            munmap(map2, dumb.size);
        }
        munmap(map, dumb.size);
        close(pfd);

        /* Re-flip a FRESH buffer after the teardown (scanout must keep
         * living content — the boot graph reallocates buffers
         * constantly) and leave it on screen while we park. */
        struct drm_mode_create_dumb dumb2;
        memset(&dumb2, 0, sizeof dumb2);
        dumb2.width = 720; dumb2.height = 1280; dumb2.bpp = 32;
        if (ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &dumb2) < 0)
            fail("CREATE_DUMB#2 (park buffer, non-fatal)", errno);
        else {
            struct drm_prime_handle exp2;
            memset(&exp2, 0, sizeof exp2);
            exp2.handle = (int32_t)dumb2.handle;
            exp2.flags = 0x80000u | 2u;
            if (ioctl(fd, DRM_IOCTL_PRIME_HANDLE_TO_FD, &exp2) < 0)
                fail("PRIME export#2 (non-fatal)", errno);
            else {
                void *m2 = mmap(NULL, dumb2.size, PROT_READ | PROT_WRITE,
                                MAP_SHARED, exp2.fd, 0);
                if (m2 == MAP_FAILED) fail("mmap#2 (non-fatal)", errno);
                else {
                    uint32_t *q = (uint32_t *)m2;
                    for (uint32_t y = 0; y < 1280; y++)
                        for (uint32_t x = 0; x < 720; x++)
                            q[y * (dumb2.pitch / 4) + x] =
                                ((uint32_t)(x * 255 / 719) << 24) | ((uint32_t)(y * 255 / 1279) << 16) | 128u << 8;
                    munmap(m2, dumb2.size);
                }
                struct drm_prime_handle imp2;
                memset(&imp2, 0, sizeof imp2);
                imp2.fd = exp2.fd;
                if (ioctl(fd, DRM_IOCTL_PRIME_FD_TO_HANDLE, &imp2) == 0) {
                    struct drm_mode_fb_cmd2 fb2;
                    memset(&fb2, 0, sizeof fb2);
                    fb2.width = 720; fb2.height = 1280;
                    fb2.pixel_format = DRM_FORMAT_XBGR8888;
                    fb2.handles[0] = (uint32_t)imp2.handle;
                    fb2.pitches[0] = dumb2.pitch;
                    if (ioctl(fd, DRM_IOCTL_MODE_ADDFB2, &fb2) == 0) {
                        a.count_objs = 1; objs[0] = planes[0];
                        counts[0] = 2;
                        props[0] = plane_fb_id; values[0] = fb2.fb_id;
                        props[1] = plane_crtc_id_id; values[1] = crtcs[0];
                        if (ioctl(fd, DRM_IOCTL_MODE_ATOMIC, &a) < 0)
                            fail("final flip (non-fatal)", errno);
                        else ok("final flip left on scanout");
                    }
                }
                /* keep exp2.fd open: closing would drop the last GEM ref */
            }
        }
    }

    close(fd);
out:
    puts_("DRM_PROBE RESULT: ");
    puts_(passed == total ? "PASS " : "FAIL ");
    putu_((unsigned long)passed); puts_("/"); putu_((unsigned long)total); puts_("\n");
    /* park so the serial keeps flushing and the boot script can screendump */
    syscall(SYS_pause);
    return 0;
}
