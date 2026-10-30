/* scanpaint — userspace scanout proof painter for the Rux virtio-gpu DRM.
 *
 * Proves the full userspace -> kernel scanout chain with NO oracle help:
 *   open card0 -> GETRESOURCES/GETCONNECTOR -> CREATE_DUMB x2 -> mmap
 *   -> paint -> ADDFB2 -> PAGE_FLIP (present_fb) at ~30 fps forever.
 *
 * The moving pattern is deliberate: a screendump that catches a stable
 * banded pattern + shifted white diagonal stripe across consecutive
 * shots proves LIVE userspace presentation, not a leftover clear.
 *
 * Built as a static musl x86_64 binary, injected via a private ramdisk:
 *   /init -> sh wrapper: /scanpaint ... & exec /init.real
 * It waits up to 180 s for /dev/dri/card0 (OH init creates devfs late),
 * logs every step to stderr (script redirects to /dev/console).
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/ioctl.h>
#include <sys/time.h>
#include <errno.h>
#include <time.h>
#include <stdarg.h>

#define DRM_IOCTL_BASE 'd'
#define DRM_IOW(nr, size) (_IOW(DRM_IOCTL_BASE, nr, size))
#define DRM_IOWR(nr, size) (_IOWR(DRM_IOCTL_BASE, nr, size))

struct drm_mode_card_res {
	uint64_t fb_id_ptr, crtc_id_ptr, connector_id_ptr, encoder_id_ptr;
	uint32_t count_fbs, count_crtcs, count_connectors, count_encoders;
	uint32_t min_width, max_width, min_height, max_height;
};

struct drm_mode_fb_cmd2 {
	uint32_t fb_id, width, height, pixel_format;
	uint32_t flags;
	uint32_t handles[4];
	uint32_t pitches[4];
	uint32_t offsets[4];
	uint64_t modifier[4];
};

struct drm_mode_crtc {
	uint64_t set_connectors_ptr;
	uint32_t count_connectors, crtc_id, fb_id;
	uint32_t x, y, gamma_size;
	uint32_t mode_valid;
	struct drm_mode_modeinfo { uint32_t clock, hdisplay, hsync_start, hsync_end,
		htotal, hskew, vdisplay, vsync_start, vsync_end, vtotal, vscan;
		uint32_t flags; uint32_t type; char name[32]; } mode;
};

struct drm_mode_map_dumb { uint32_t handle, pad; uint64_t offset; };

struct drm_mode_create_dumb {
	uint32_t height, width, bpp, flags;
	uint32_t handle, pitch; uint64_t size;
};

struct drm_mode_modeinfo_pad { /* sizeof 68 in kernel DrmModeInfo guess; GETCONNECTOR only */
	uint32_t v[17];
};

#define DRM_IOCTL_MODE_GETRESOURCES DRM_IOWR(0xa0, struct drm_mode_card_res)
#define DRM_IOCTL_MODE_GETCONNECTOR DRM_IOWR(0xa7, struct drm_mode_connector)
struct drm_mode_connector {
	uint64_t encoders_ptr, modes_ptr, props_ptr, prop_values_ptr;
	uint32_t count_modes, count_props, count_encoders;
	uint32_t encoder_id, connector_id, connector_type, connector_type_id;
	uint32_t connection, mm_width, mm_height, subpixel, pad;
};
#define DRM_IOCTL_MODE_CREATE_DUMB DRM_IOWR(0xb2, struct drm_mode_create_dumb)
#define DRM_IOCTL_MODE_MAP_DUMB   DRM_IOWR(0xb3, struct drm_mode_map_dumb)
#define DRM_IOCTL_MODE_ADDFB2     DRM_IOWR(0xb8, struct drm_mode_fb_cmd2)
#define DRM_IOCTL_MODE_RMFB       DRM_IOR(0xaf, uint32_t) /* upstream libdrm encoding */
struct drm_mode_crtc_page_flip { uint32_t crtc_id, fb_id; uint32_t flags, reserved; uint64_t user_data; };
#define DRM_IOCTL_MODE_PAGE_FLIP  DRM_IOW(0xb0, struct drm_mode_crtc_page_flip) /* upstream libdrm encoding */

#define FOURCC_XR24 0x34325258u /* XR24 little-endian */

static int kmsg_fd = -1;

static void klog(const char *fmt, ...)
{
	char buf[256];
	va_list ap;
	va_start(ap, fmt);
	int n = vsnprintf(buf, sizeof(buf) - 1, fmt, ap);
	va_end(ap);
	if (n > 0) {
		buf[n] = '\n';
		if (kmsg_fd >= 0)
			write(kmsg_fd, buf, n + 1);
		fprintf(stderr, "%s", buf);
	}
}

static long long now_ms(void)
{
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return (long long)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static int xioctl(int fd, unsigned long req, void *a, const char *what)
{
	int r = ioctl(fd, req, a);
	if (r < 0)
		klog("scanpaint: %s -> %d errno=%d (%s)", what, r, errno, strerror(errno));
	else
		klog("scanpaint: %s ok", what);
	return r;
}

static void paint(uint32_t *fb, int w, int h, int pitch_px, int frame)
{
	static const uint32_t bars[8] = {
		0xff0000ff, 0xff00ff00, 0xffff0000, 0xff00ffff,
		0xffffff00, 0xffff00ff, 0xff404040, 0xffe0e0e0,
	};
	for (int y = 0; y < h; y++) {
		uint32_t *row = (uint32_t *)((uint8_t *)fb + (size_t)y * pitch_px * 4);
		for (int x = 0; x < w; x++) {
			uint32_t c = bars[((x / 90) & 7)];
			if ((((x + y) / 16) + frame) % 12 == 0)
				c = 0xffffffff; /* moving white diagonal stripe */
			if (y >= h / 2 - 2 && y <= h / 2 + 2)
				c = 0xff000000 | (uint32_t)((frame * 40) & 0xffffff); /* frame counter band */
			row[x] = c;
		}
	}
}

int main(int argc, char **argv)
{
	int w = argc > 1 ? atoi(argv[1]) : 720;
	int h = argc > 2 ? atoi(argv[2]) : 1280;
	long long deadline = now_ms() + 180000;

	/* Mirror logs to the serial console once devfs provides it. */
	{
		int cfd = -1;
		for (int i = 0; i < 200 && cfd < 0; i++) {
			cfd = open("/dev/console", O_WRONLY);
			if (cfd < 0) usleep(100000);
		}
		if (cfd >= 0) { dup2(cfd, 1); dup2(cfd, 2); }
	}
	for (int i = 0; i < 100 && kmsg_fd < 0; i++) {
		kmsg_fd = open("/dev/kmsg", O_WRONLY);
		if (kmsg_fd < 0) usleep(100000);
	}
	setvbuf(stderr, NULL, _IONBF, 0);
	klog("scanpaint: start %dx%d pid=%d kmsgfd=%d", w, h, getpid(), kmsg_fd);
	int fd = -1;
	int polls = 0;
	while (now_ms() < deadline) {
		fd = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
		if (fd >= 0)
			break;
		if ((polls++ % 20) == 0)
			klog("scanpaint: polling /dev/dri/card0 errno=%d", errno);
		usleep(100000);
	}
	if (fd < 0) {
		klog("scanpaint: FATAL no /dev/dri/card0 after 180s");
		return 1;
	}
	klog("scanpaint: card0 open fd=%d", fd);

	struct drm_mode_card_res res;
	memset(&res, 0, sizeof(res));
	uint32_t crtcs[4], conns[4];
	res.crtc_id_ptr = (uint64_t)crtcs;
	res.connector_id_ptr = (uint64_t)conns;
	res.count_crtcs = 4;
	res.count_connectors = 4;
	if (xioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res, "GETRESOURCES") < 0)
		return 1;
	klog("scanpaint: crtcs=%u conns=%u crtc0=%u conn0=%u",
		res.count_crtcs, res.count_connectors, crtcs[0], conns[0]);

	struct drm_mode_connector conn;
	memset(&conn, 0, sizeof(conn));
	uint32_t encs[2];
	static struct { uint32_t v[17]; } modes[2];
	conn.encoders_ptr = (uint64_t)encs;
	conn.modes_ptr = (uint64_t)modes;
	conn.count_encoders = 2;
	conn.count_modes = 2;
	conn.connector_id = conns[0];
	if (xioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, &conn, "GETCONNECTOR") < 0)
		return 1;
	klog("scanpaint: conn status=%u modes=%u mode0=%ux%u",
		conn.connection, conn.count_modes, modes[0].v[1], modes[0].v[6]);

	uint32_t handles[2], pitches[2];
	uint64_t sizes[2] __attribute__((unused));
	uint32_t *cpu[2];
	uint32_t fbs[2];
	struct drm_mode_fb_cmd2 fbreq;
	for (int i = 0; i < 2; i++) {
		struct drm_mode_create_dumb cd;
		memset(&cd, 0, sizeof(cd));
		cd.width = w;
		cd.height = h;
		cd.bpp = 32;
		if (xioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &cd, "CREATE_DUMB") < 0)
			return 1;
		handles[i] = cd.handle;
		pitches[i] = cd.pitch;
		sizes[i] = cd.size;
		klog("scanpaint: dumb[%d] handle=%u pitch=%u size=%llu",
			i, cd.handle, cd.pitch, (unsigned long long)cd.size);

		struct drm_mode_map_dumb md;
		memset(&md, 0, sizeof(md));
		md.handle = cd.handle;
		if (xioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &md, "MAP_DUMB") < 0)
			return 1;
		cpu[i] = mmap(NULL, cd.size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, md.offset);
		if (cpu[i] == MAP_FAILED) {
			klog("scanpaint: mmap failed errno=%d", errno);
			return 1;
		}
		memset(cpu[i], 0, cd.size);

		memset(&fbreq, 0, sizeof(fbreq));
		fbreq.width = w;
		fbreq.height = h;
		fbreq.pixel_format = FOURCC_XR24;
		fbreq.handles[0] = handles[i];
		fbreq.pitches[0] = cd.pitch;
		if (xioctl(fd, DRM_IOCTL_MODE_ADDFB2, &fbreq, "ADDFB2") < 0)
			return 1;
		fbs[i] = fbreq.fb_id;
		klog("scanpaint: fb[%d]=%u", i, fbs[i]);
	}

	/* First frame: present fb0. */
	paint(cpu[0], w, h, pitches[0] / 4, 0);
	struct drm_mode_crtc_page_flip flip;
	memset(&flip, 0, sizeof(flip));
	flip.crtc_id = crtcs[0];
	flip.fb_id = fbs[0];
	if (xioctl(fd, DRM_IOCTL_MODE_PAGE_FLIP, &flip, "PAGE_FLIP#0") < 0)
		return 1;
	klog("scanpaint: PATTERN LIVE");

	int frame = 1;
	int cur = 0;
	while (1) {
		struct timespec ts = { 0, 250 * 1000 * 1000 }; /* 4 fps proof cadence */
		nanosleep(&ts, NULL);
		cur ^= 1;
		paint(cpu[cur], w, h, pitches[cur] / 4, frame);
		memset(&flip, 0, sizeof(flip));
		flip.crtc_id = crtcs[0];
		flip.fb_id = fbs[cur];
		if (xioctl(fd, DRM_IOCTL_MODE_PAGE_FLIP, &flip, "PAGE_FLIP") < 0) {
			klog("scanpaint: flip loop broke, retry in 1s");
			sleep(1);
			continue;
		}
		if ((frame % 30) == 0)
			klog("scanpaint: frame %d presented", frame);
		frame++;
	}
	return 0;
}
