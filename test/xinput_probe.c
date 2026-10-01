/* xinput_probe — raw X11 protocol input probe (no libX11), same style as
 * the xprobe render probe. Connects to /tmp/.X11-unix/X0, creates+maps a
 * full-screen window selecting key/button/motion/exposure events, grabs
 * keyboard focus, then prints every delivered event to stdout (serial).
 *
 * Expected event sequence for the QMP injection script:
 *   key a down/up   (Linux KEY_A=30 -> X keycode 38)
 *   key b down/up   (Linux KEY_B=48 -> X keycode 56)
 *   abs moves       -> MotionNotify
 *   btn left down/up-> ButtonPress/ButtonRelease
 *
 * Two details found the hard way during bring-up:
 * - CreateWindow attribute bit for background-PIXEL is 0x2 (0x1 selects
 *   background-PIXMAP -> BadPixmap), and SetInputFocus packs revert-to in
 *   byte 1 with request length 3 (not a trailing field).
 * - AutoRepeatMode is turned OFF: the injector holds keys for seconds and
 *   the server's key repeat otherwise floods dozens of extra KeyPress
 *   events that mask the real down/up pairs.
 * Build: riscv64-linux-gnu-gcc -static -O2 -Wall -o xinput_probe xinput_probe.c
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <signal.h>
#include <errno.h>
#include <stdint.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>

static int fd;
static uint8_t out[512];
static int outlen;
static uint32_t rid_base, rid_mask, root_win;
static uint16_t root_w, root_h;

static void wx8(uint8_t v) { out[outlen++] = v; }
static void wx16(uint16_t v) { wx8(v & 0xff); wx8(v >> 8); }
static void wx32(uint32_t v) { wx16(v & 0xffff); wx16(v >> 16); }
static void sendx(void) {
    uint8_t *p = out; int n = outlen;
    while (n > 0) {
        int k = write(fd, p, n);
        if (k <= 0) { printf("xinput_probe: write err errno=%d\n", errno); fflush(stdout); exit(1); }
        p += k; n -= k;
    }
    outlen = 0;
}

static uint8_t rd[64];
static void rdn(int n) {
    int got = 0;
    while (got < n) {
        int k = read(fd, rd + got, n - got);
        if (k <= 0) { printf("xinput_probe: read EOF/err errno=%d\n", errno); fflush(stdout); exit(1); }
        got += k;
    }
}
static uint32_t ru32(const uint8_t *p) { return p[0] | (p[1] << 8) | ((uint32_t)(p[2] | (p[3] << 8)) << 16); }
static uint16_t ru16(const uint8_t *p) { return p[0] | (p[1] << 8); }

/* consume events until the next reply (rd holds its 32-byte header) */
static void wait_reply(void) {
    for (;;) {
        rdn(32);
        if (rd[0] == 1)
            return;
        if (rd[0] == 0)
            printf("xinput_probe: XERROR code=%u seq=%u bad=0x%x major=%u minor=%u\n",
                   rd[1], ru16(rd + 2), ru32(rd + 4), rd[8], rd[9]);
        else
            printf("xinput_probe: (event %u while waiting reply)\n", rd[0]);
        fflush(stdout);
    }
}
static void read_exact(uint8_t *buf, uint32_t n) {
    uint32_t got = 0;
    while (got < n) { int k = read(fd, buf + got, n - got); if (k <= 0) exit(1); got += k; }
}

static const char *evname(uint8_t t) {
    switch (t) {
    case 2: return "KeyPress";
    case 3: return "KeyRelease";
    case 4: return "ButtonPress";
    case 5: return "ButtonRelease";
    case 6: return "MotionNotify";
    case 7: return "EnterNotify";
    case 8: return "LeaveNotify";
    case 9: return "FocusIn";
    case 10: return "FocusOut";
    case 12: return "Expose";
    case 19: return "MapNotify";
    case 22: return "ConfigureNotify";
    default: return "?";
    }
}

static void on_alarm(int s) {
    (void)s;
    const char msg[] = "xinput_probe: TIMEOUT\n";
    ssize_t ign = write(1, msg, sizeof msg - 1);
    (void)ign;
    _exit(3);
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    signal(SIGALRM, on_alarm);
    alarm(900);

    struct sockaddr_un sa;
    memset(&sa, 0, sizeof sa);
    sa.sun_family = AF_UNIX;
    strcpy(sa.sun_path, "/tmp/.X11-unix/X0");

    /* Xorg full init is slow under TCG: retry the connect for up to 4 min */
    int i;
    for (i = 0; i < 120; i++) {
        fd = socket(AF_UNIX, SOCK_STREAM, 0);
        if (fd >= 0 && connect(fd, (struct sockaddr *)&sa, sizeof sa) == 0)
            break;
        if (fd >= 0) close(fd);
        if (i % 15 == 0) { printf("xinput_probe: waiting for X0 (try %d)\n", i); fflush(stdout); }
        sleep(2);
    }
    if (i == 120) { printf("xinput_probe: connect FAILED\n"); return 1; }
    printf("xinput_probe: connected\n");

    /* connection setup */
    outlen = 0;
    wx8('l'); wx8(0); wx16(11); wx16(0); wx16(0); wx16(0); wx16(0);
    sendx();
    rdn(8);
    if (rd[0] != 1) { printf("xinput_probe: setup FAILED resp=%u\n", rd[0]); return 1; }
    uint32_t extra = ru16(rd + 6);
    uint8_t *body = malloc(extra * 4 + 8);
    read_exact(body, extra * 4);
    rid_base = ru32(body + 4);
    rid_mask = ru32(body + 8);
    uint16_t vlen = ru16(body + 16);
    uint8_t nformats = body[21];
    uint8_t *scr = body + 32 + ((vlen + 3) & ~3u) + nformats * 8;
    root_win = ru32(scr + 0);
    root_w = ru16(scr + 20);
    root_h = ru16(scr + 22);
    uint8_t motion_bu = body[12]; /* largest motion buffer, diagnostics */
    printf("xinput_probe: setup OK root=0x%x %ux%u motionbuf=%u\n",
           root_win, root_w, root_h, motion_bu);

    uint32_t wid = (rid_base & ~rid_mask) | (0x350000 & rid_mask);

    /* create full-screen window; attr bits (Xproto.h): CWBackPixel=0x2,
     * CWEventMask=0x800 (2 values, request length 10 words) */
    uint32_t mask = 0x1 | 0x2 | 0x4 | 0x8 | 0x40 | 0x10 | 0x20 |
                    0x8000 | 0x20000 | 0x200000;
    printf("xinput_probe: create-window 0x%x mask=0x%x\n", wid, mask);
    outlen = 0;
    wx8(1); wx8(0); wx16(10);
    wx32(wid); wx32(root_win);
    wx16(0); wx16(0); wx16(root_w); wx16(root_h); wx16(0);
    wx16(1);            /* InputOutput */
    wx32(0);            /* CopyFromParent visual */
    wx32(0x2 | 0x800);
    wx32(0x3050a0);     /* background-pixel */
    wx32(mask);         /* event-mask */
    sendx();

    printf("xinput_probe: map-window\n");
    outlen = 0; wx8(8); wx8(0); wx16(2); wx32(wid); sendx();

    /* GetInputFocus as a sync ping that also tells us the current focus */
    outlen = 0; wx8(43); wx8(0); wx16(1); sendx();
    wait_reply();
    printf("xinput_probe: focus now=0x%x\n", ru32(rd + 8));

    /* SetInputFocus(wid, CurrentTime, RevertToParent): keys come to us
     * even with no WM running. Layout: [42][revert-to][len=3][focus][time] */
    outlen = 0; wx8(42); wx8(2); wx16(3); wx32(wid); wx32(0); sendx();
    outlen = 0; wx8(43); wx8(0); wx16(1); sendx();
    wait_reply();
    printf("xinput_probe: focus after set=0x%x\n", ru32(rd + 8));

    /* WarpPointer to window center so motion/button hit our window */
    outlen = 0; wx8(41); wx8(0); wx16(6);
    wx32(0); wx32(wid); wx16(0); wx16(0); wx16(0); wx16(0);
    wx16(root_w / 2); wx16(root_h / 2); sendx();

    /* ChangeKeyboardControl: AutoRepeatMode=Off (bit 7, value 0=Off).
     * The injector holds keys for seconds; DIX autorepeat would otherwise
     * flood repeated KeyPress events and mask the real down/up pairs. */
    outlen = 0; wx8(102); wx8(0); wx16(3);
    wx32(0x80); wx8(0); wx8(0); wx8(0); wx8(0); sendx();

    /* one more sync ping: flush everything, then we are event-only */
    outlen = 0; wx8(43); wx8(0); wx16(1); sendx();
    wait_reply();
    printf("xinput_probe: XINPUT-PROBE-READY (keycode=linux+8; KEY_A=30->38)\n");
    alarm(0);

    /* event loop: everything we selected is a 32-byte event */
    long nkey = 0, nbtn = 0, nmot = 0, noth = 0;
    struct timespec t0ev;
    clock_gettime(CLOCK_MONOTONIC, &t0ev);
    for (;;) {
        rdn(32);
        uint8_t t = rd[0];
        uint16_t rx = ru16(rd + 20), ry = ru16(rd + 22);
        uint16_t ex = ru16(rd + 24), ey = ru16(rd + 26);
        uint16_t st = ru16(rd + 28);
        struct timespec now;
        clock_gettime(CLOCK_MONOTONIC, &now);
        long ms = (now.tv_sec - t0ev.tv_sec) * 1000 +
                  (now.tv_nsec - t0ev.tv_nsec) / 1000000;
        switch (t) {
        case 2: case 3:
            nkey++;
            printf("xinput_probe: EVT +%ldms %s keycode=%u state=0x%x sts=%u\n",
                   ms, evname(t), rd[1], st, ru32(rd + 4));
            break;
        case 4: case 5:
            nbtn++;
            printf("xinput_probe: EVT +%ldms %s button=%u root=(%u,%u) win=(%u,%u) state=0x%x\n",
                   ms, evname(t), rd[1], rx, ry, ex, ey, st);
            break;
        case 6:
            nmot++;
            printf("xinput_probe: EVT +%ldms MotionNotify root=(%u,%u) win=(%u,%u) state=0x%x\n",
                   ms, rx, ry, ex, ey, st);
            break;
        case 12:
            printf("xinput_probe: EVT +%ldms Expose %ux%u count=%u\n",
                   ms, ru16(rd + 12), ru16(rd + 14), ru16(rd + 16));
            break;
        default:
            noth++;
            printf("xinput_probe: EVT +%ldms %s detail=%u\n", ms, evname(t), rd[1]);
            break;
        }
        if ((nkey + nbtn + nmot + noth) % 50 == 0)
            printf("xinput_probe: tally key=%ld btn=%ld mot=%ld other=%ld\n",
                   nkey, nbtn, nmot, noth);
    }
}
