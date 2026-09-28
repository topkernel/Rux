// udesk — Ubuntu-branded native framebuffer login + desktop session for Rux.
//
// One static binary renders a GDM-style login screen and (after auth) a
// desktop with a top panel, a launch dock and three apps:
//   Terminal  — a real /bin/dash shell piped into an on-screen window
//   SysInfo   — /proc-driven system information panel
//   About     — Ubuntu release / session credits
//
// Input comes from the serial console (stdin), switched to raw mode so every
// keystroke arrives immediately; keys are routed to the focused control
// (login field, focused window) or to the window manager (Ctrl- shortcuts).
//
// Architecture note: Rux has a history of fcntl(F_SETFL) stalls, so the
// renderer never multiplexes fds itself. It blocks on ONE event pipe fed by
// helper processes:
//   helper-key   : blocking read(0)     -> {K, byte}
//   helper-tick  : sleep(1) loop        -> {T}          clock + cursor blink
//   helper-shell : blocking read(shell) -> {S, bytes}   terminal output
// Pipe writes <= PIPE_BUF are atomic so frames never interleave; the reader
// keeps a sticky buffer and dispatches whole frames only.

#include <unistd.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <stdio.h>
#include <string.h>
#include <stdint.h>
#include <stdlib.h>
#include <errno.h>

// ---------------------------------------------------------------- constants
#define FBIOGET_VSCREENINFO 0x4600
#define FBIO_FLUSH          0x4610
#define TCSETS              0x5402

struct fb_var { uint32_t xres, yres, xres_v, yres_v, xoff, yoff, bpp, pad[6]; };

// Ubuntu palette
#define C_AUB_DARK  0x2C001EU /* aubergine */
#define C_AUB_MID   0x772953U
#define C_AUB_DEEP  0x1B0410U
#define C_ORANGE    0xE95420U
#define C_WHITE     0xFFFFFFU
#define C_TEXT_DIM  0xB9AEB9U
#define C_TERM_BG   0x300A24U /* classic Ubuntu terminal purple */
#define C_TERM_FG   0xEDEAE6U
#define C_GREEN     0x8AE234U
#define C_ERROR     0xF15A5AU
#define C_CARD_BG   0x3A0F2EU
#define C_FIELD_BG  0x23091BU
#define C_TITLE_A   0x5E2750U /* active title bar */
#define C_TITLE_I   0x33202CU /* inactive title bar */
#define C_DOCK_BTN  0x4A2540U

// apps
#define APP_TERM  0
#define APP_INFO  1
#define APP_ABOUT 2
#define NAPPS     3

// event pipe frame: type(1) len(1) data(len)
#define EV_KEY   'K'
#define EV_TICK  'T'
#define EV_SHELL 'S'
#define EV_MAX   256

// public-domain font8x8 (ASCII 32..126)
static const uint8_t F8[95][8] = {
{0,0,0,0,0,0,0,0},{24,60,60,60,24,126,126,0},{0x33,0x66,0xcc,0x66,0x33,0,0,0},
{0x36,0x7f,0x7f,0x36,0x1c,0x63,0x7f,0},{0x60,0x66,0x0c,0x18,0x30,0x66,0x06,0},
{0x38,0x6c,0x6c,0x38,0x76,0xdc,0xcc,0},{0,0,0,0,0,0,0,0},{0x18,0x18,0x3c,0x3c,0x18,0x18,0,0},
{0x66,0x66,0x22,0x22,0,0,0,0},{0x66,0xff,0xff,0x66,0x66,0,0,0},{0x18,0x3e,0x60,0x3c,0x06,0x7c,0x18,0},
{0x62,0x66,0x0c,0x18,0x30,0x66,0x46,0},{0x3c,0x66,0x3c,0x38,0x67,0x66,0x3f,0},{6,6,0xc,0,0,0,0,0},
{0x0c,0x18,0x30,0x30,0x30,0x18,0x0c,0},{0x30,0x18,0x0c,0x0c,0x0c,0x18,0x30,0},{0,0x66,0x3c,0xff,0x3c,0x66,0,0},
{0,0x18,0x18,0x7e,0x18,0x18,0,0},{0,0,0,0,0,0x18,0x18,0x30},{0,0,0,0x7e,0,0,0,0},
{0,0,0,0,0,0x18,0x18,0},{0,3,6,0xc,0x18,0x30,0x60,0},{0x3c,0x66,0x6e,0x76,0x66,0x66,0x3c,0},
{0x18,0x38,0x18,0x18,0x18,0x18,0x7e,0},{0x3c,0x66,0x06,0x1c,0x30,0x60,0x7e,0},{0x3c,0x66,0x06,0x1c,0x06,0x66,0x3c,0},
{0x0e,0x1e,0x36,0x66,0x7f,0x06,0x06,0},{0x7e,0x60,0x7c,0x06,0x06,0x66,0x3c,0},{0x3c,0x66,0x60,0x7c,0x66,0x66,0x3c,0},
{0x7e,0x66,0x0c,0x18,0x18,0x18,0x18,0},{0x3c,0x66,0x66,0x3c,0x66,0x66,0x3c,0},{0x3c,0x66,0x66,0x3e,0x06,0x66,0x3c,0},
{0,0x18,0,0,0,0x18,0,0},{0,0x18,0,0,0,0x18,0x18,0x30},{0x0e,0x18,0x30,0x60,0x30,0x18,0x0e,0},
{0,0,0x7e,0,0x7e,0,0,0},{0x70,0x18,0x0c,0x06,0x0c,0x18,0x70,0},{0x3c,0x66,0x06,0x0c,0x18,0,0x18,0},
{0x3c,0x66,0x6e,0x6a,0x6e,0x60,0x3e,0},{0x18,0x3c,0x66,0x66,0x7e,0x66,0x66,0},{0x7c,0x66,0x66,0x7c,0x66,0x66,0x7c,0},
{0x3c,0x66,0x60,0x60,0x60,0x66,0x3c,0},{0x78,0x6c,0x66,0x66,0x66,0x6c,0x78,0},{0x7e,0x60,0x60,0x7c,0x60,0x60,0x7e,0},
{0x7e,0x60,0x60,0x7c,0x60,0x60,0x60,0},{0x3c,0x66,0x60,0x6e,0x66,0x66,0x3e,0},{0x66,0x66,0x66,0x7e,0x66,0x66,0x66,0},
{0x7e,0x18,0x18,0x18,0x18,0x18,0x7e,0},{0x06,0x06,0x06,0x06,0x06,0x66,0x3c,0},{0x66,0x6c,0x78,0x70,0x78,0x6c,0x66,0},
{0x60,0x60,0x60,0x60,0x60,0x60,0x7e,0},{0x63,0x77,0x7f,0x6b,0x63,0x63,0x63,0},{0x66,0x76,0x7e,0x7e,0x6e,0x66,0x66,0},
{0x3c,0x66,0x66,0x66,0x66,0x66,0x3c,0},{0x7c,0x66,0x66,0x7c,0x60,0x60,0x60,0},{0x3c,0x66,0x66,0x66,0x6a,0x6c,0x36,0},
{0x7c,0x66,0x66,0x7c,0x6c,0x66,0x66,0},{0x3c,0x66,0x60,0x3c,0x06,0x66,0x3c,0},{0x7e,0x18,0x18,0x18,0x18,0x18,0x18,0},
{0x66,0x66,0x66,0x66,0x66,0x66,0x7e,0},{0x66,0x66,0x66,0x66,0x66,0x3c,0x18,0},{0x63,0x63,0x63,0x6b,0x7f,0x77,0x63,0},
{0x66,0x66,0x3c,0x18,0x3c,0x66,0x66,0},{0x66,0x66,0x66,0x3c,0x18,0x18,0x18,0},{0x7e,0x06,0x0c,0x18,0x30,0x60,0x7e,0},
{0x3c,0x30,0x30,0x30,0x30,0x30,0x3c,0},{0x0c,0x12,0x30,0x7c,0x30,0x62,0xfc,0},{0x3c,0x0c,0x0c,0x0c,0x0c,0x0c,0x3c,0},
{0,0,0x24,0x66,0xff,0x66,0x24,0},{0,0x10,0x38,0x7c,0x38,0x10,0,0},{0,0,0,0,0,0,0,0},
{0x18,0x30,0x60,0x60,0x60,0x30,0x18,0},{0x66,0x66,0x66,0x66,0x66,0,0,0},{0x18,0x18,0x7e,0x18,0x18,0,0x7e,0},
{0x1c,0x30,0x60,0x30,0x1c,0,0x7e,0},{0x30,0x18,0x0c,0x18,0x30,0,0x7e,0},{0x30,0x30,0x30,0x30,0x30,0,0x7e,0},
{0,0,0x3c,0x3c,0x3c,0x3c,0,0},{0,0,0,0,0,0,0,0},{0,0,0,0,0,0,0,0},
};

// ------------------------------------------------------------------- fb glue
static uint32_t *fb; static int W, H, fbfd;
// measured mapping on Rux virtio-gpu: screen R<-V[15:8], G<-V[23:16], B<-V[31:24]
static uint32_t rgb(uint32_t hex) {
    return ((hex >> 16 & 0xff) << 8) | ((hex >> 8 & 0xff) << 16) | ((hex & 0xff) << 24);
}
static uint32_t lerp(uint32_t a, uint32_t b, int t, int n) { // hex-color lerp
    int ar = a >> 16 & 0xff, ag = a >> 8 & 0xff, ab = a & 0xff;
    int br = b >> 16 & 0xff, bg = b >> 8 & 0xff, bb = b & 0xff;
    uint32_t r = ar + (br - ar) * t / n, g = ag + (bg - ag) * t / n, bl = ab + (bb - ab) * t / n;
    return rgb(r << 16 | g << 8 | bl);
}
static void px(int x, int y, uint32_t v) { if (x >= 0 && x < W && y >= 0 && y < H) fb[y * W + x] = v; }
static void rect(int x, int y, int w, int h, uint32_t v) {
    for (int j = y; j < y + h; j++) for (int i = x; i < x + w; i++) px(i, j, v);
}
static void hline(int x, int y, int w, uint32_t v) { for (int i = x; i < x + w; i++) px(i, y, v); }
static void vline(int x, int y, int h, uint32_t v) { for (int j = y; j < y + h; j++) px(x, j, v); }
static void frame(int x, int y, int w, int h, uint32_t v) {
    hline(x, y, w, v); hline(x, y + h - 1, w, v); vline(x, y, h, v); vline(x + w - 1, y, h, v);
}
static void flushfb(void) { ioctl(fbfd, FBIO_FLUSH, 0); }

// glyph renderers; *_t variants leave background pixels untouched (overdraw-safe)
static int draw_char_t(int x, int y, uint8_t ch, uint32_t fg, int xs, int ys) {
    if (ch < 32 || ch > 126) ch = '?';
    const uint8_t *gl = F8[ch - 32];
    for (int r = 0; r < 8; r++)
        for (int c = 0; c < 8; c++)
            if ((gl[r] >> (7 - c)) & 1)
                for (int sy = 0; sy < ys; sy++)
                    for (int sx = 0; sx < xs; sx++)
                        px(x + c * xs + sx, y + r * ys + sy, fg);
    return 8 * xs;
}
static int draw_char(int x, int y, uint8_t ch, uint32_t fg, uint32_t bg, int xs, int ys) {
    if (ch < 32 || ch > 126) ch = '?';
    const uint8_t *gl = F8[ch - 32];
    for (int r = 0; r < 8; r++)
        for (int c = 0; c < 8; c++) {
            uint32_t v = (gl[r] >> (7 - c)) & 1 ? fg : bg;
            for (int sy = 0; sy < ys; sy++)
                for (int sx = 0; sx < xs; sx++)
                    px(x + c * xs + sx, y + r * ys + sy, v);
        }
    return 8 * xs;
}
static void draw_text(int x, int y, const char *s, uint32_t fg, uint32_t bg, int xs, int ys) {
    while (*s) x += draw_char(x, y, (uint8_t)*s++, fg, bg, xs, ys) + xs;
}
static void draw_text_t(int x, int y, const char *s, uint32_t fg, int xs, int ys) {
    while (*s) x += draw_char_t(x, y, (uint8_t)*s++, fg, xs, ys) + xs;
}
static int text_w(const char *s, int xs) { return (int)strlen(s) * 9 * xs; }
static void draw_text_ctr(int cx, int y, const char *s, uint32_t fg, uint32_t bg, int xs, int ys) {
    draw_text(cx - text_w(s, xs) / 2, y, s, fg, bg, xs, ys);
}
static void draw_text_ctr_t(int cx, int y, const char *s, uint32_t fg, int xs, int ys) {
    draw_text_t(cx - text_w(s, xs) / 2, y, s, fg, xs, ys);
}

// Ubuntu "circle of friends": ring + three head dots
static void draw_ubuntu_logo(int cx, int cy, int r, uint32_t col) {
    static const int ang[3][2] = {{0, -100}, {-87, 50}, {87, 50}}; // ~unit vectors (deg-ish)
    int hd = r / 3 + 1 < 3 ? 3 : r / 3 + 1;
    int ri = r - r / 3 + 1;
    for (int y = -r - hd - 2; y <= r + hd + 2; y++)
        for (int x = -r - hd - 2; x <= r + hd + 2; x++) {
            int d = x * x + y * y;
            if (d <= r * r && d >= ri * ri) { px(cx + x, cy + y, col); continue; }
            for (int k = 0; k < 3; k++) {
                int ox = ang[k][0] * r / 100, oy = ang[k][1] * r / 100;
                int dx = x - ox, dy = y - oy;
                if (dx * dx + dy * dy <= hd * hd) { px(cx + x, cy + y, col); break; }
            }
        }
}

// ------------------------------------------------------------------- UI state
static int state_login = 1;
static char user[40], pass[40];
static int ulen, plen, field; // 0=username 1=password
static int authfail, blink;
static unsigned ticks; // seconds since session start

static int win_open[NAPPS], focus = APP_TERM;

#define TCOLS 48
#define TROWS 17
static uint8_t tgrid[TROWS][TCOLS];
static int tcx, tcy, tesc;

struct wingeo { int x, y, w, h; const char *title; };
static struct wingeo WGE[NAPPS];
#define TITLE_H 22
#define DOCK_W 58
#define PANEL_H 28

// window layout: wide screens get a side-by-side row, narrow ones a cascade
static void init_wge(void) {
    if (W >= 1240) {
        int x0 = (W - 1200) / 2;
        WGE[APP_TERM]  = (struct wingeo){x0,       110, 460, 330, "Terminal — root@rux-ubuntu"};
        WGE[APP_INFO]  = (struct wingeo){x0 + 500, 140, 360, 250, "System Info"};
        WGE[APP_ABOUT] = (struct wingeo){x0 + 900, 170, 300, 230, "About Ubuntu"};
    } else {
        WGE[APP_TERM]  = (struct wingeo){70, 60, 460, 330, "Terminal — root@rux-ubuntu"};
        WGE[APP_INFO]  = (struct wingeo){120, 90, 360, 250, "System Info"};
        WGE[APP_ABOUT] = (struct wingeo){180, 120, 300, 230, "About Ubuntu"};
    }
}

static char kver[44] = "n/a", upstr[32] = "n/a", memstr[32] = "n/a";

static void read_line_from(const char *path, char *dst, int n) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return;
    int r = read(fd, dst, n - 1);
    close(fd);
    if (r <= 0) { dst[0] = 0; return; }
    dst[r] = 0;
    char *nl = strchr(dst, '\n'); if (nl) *nl = 0;
}
static void sysinfo_refresh(void) {
    char buf[128];
    read_line_from("/proc/version", kver, sizeof kver);
    if (!kver[0]) strcpy(kver, "Rux OS (RISC-V 64)");
    read_line_from("/proc/uptime", buf, sizeof buf);
    if (buf[0]) {
        char *sp = strchr(buf, ' '); if (sp) *sp = 0;
        long up = atol(buf);
        snprintf(upstr, sizeof upstr, "%ldm %lds", up / 60, up % 60);
    } else strcpy(upstr, "n/a");
    read_line_from("/proc/meminfo", buf, sizeof buf);
    char *col = strchr(buf, ':');
    if (col) snprintf(memstr, sizeof memstr, "%ld MB", atol(col + 1) / 1024);
    else strcpy(memstr, "n/a");
}

// ------------------------------------------------------------- terminal model
static void term_putc(uint8_t ch) {
    if (tesc) { // swallow ANSI escape sequences emitted by the shell
        if ((ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z')) tesc = 0;
        return;
    }
    if (ch == 0x1b) { tesc = 1; return; }
    if (ch == '\n') { tcx = 0; tcy++; }
    else if (ch == '\r') tcx = 0;
    else if (ch == '\b') { if (tcx > 0) { tcx--; tgrid[tcy][tcx] = ' '; } }
    else if (ch == '\t') { do { tgrid[tcy][tcx] = ' '; tcx++; } while (tcx % 4 && tcx < TCOLS); }
    else if (ch >= 32 && ch < 127) { tgrid[tcy][tcx] = ch; tcx++; }
    else return;
    if (tcx >= TCOLS) { tcx = 0; tcy++; }
    if (tcy >= TROWS) {
        memmove(tgrid, tgrid[1], (TROWS - 1) * TCOLS);
        memset(tgrid[TROWS - 1], ' ', TCOLS);
        tcy = TROWS - 1;
    }
}

// ------------------------------------------------------------------ painters
static void paint_bg(int y0, int y1) {
    for (int y = y0; y < y1; y++)
        for (int x = 0; x < W; x++)
            px(x, y, lerp(C_AUB_DARK, C_AUB_MID, y - y0, y1 - y0));
}

static void paint_field(int x, int y, int w, const char *lbl, const char *txt,
                        int active, int masked, int cur) {
    draw_text(x + 4, y, lbl, C_TEXT_DIM, C_CARD_BG, 1, 1);
    int fy = y + 12;
    rect(x, fy, w, 26, C_FIELD_BG);
    frame(x, fy, w, 26, active ? rgb(C_ORANGE) : rgb(C_AUB_MID));
    char disp[40];
    if (masked) { int n = (int)strlen(txt); if (n > 38) n = 38; memset(disp, '*', (size_t)n); disp[n] = 0; }
    else snprintf(disp, sizeof disp, "%.38s", txt);
    int dx = x + 8;
    for (const char *p = disp; *p; p++)
        dx += draw_char(dx, fy + 5, (uint8_t)*p, rgb(C_WHITE), rgb(C_FIELD_BG), 1, 2) + 1;
    if (active && cur) rect(dx + 1, fy + 5, 8, 16, rgb(C_ORANGE));
}

static void paint_login(void) {
    paint_bg(0, H);
    int ly = H / 5; // logo anchor, proportional
    draw_ubuntu_logo(W / 2, ly, 34, rgb(C_WHITE));
    draw_text_ctr_t(W / 2, ly + 52, "ubuntu", rgb(C_WHITE), 3, 3);
    draw_text_ctr_t(W / 2, ly + 86, "on Rux OS  (RISC-V 64)", rgb(C_ORANGE), 1, 1);

    int cw = 340, cx = W / 2 - cw / 2, cy = H * 45 / 100;
    rect(cx, cy, cw, 132, rgb(C_CARD_BG));
    frame(cx, cy, cw, 132, rgb(C_AUB_MID));
    paint_field(cx + 14, cy + 14, cw - 28, "Username", user, field == 0, 0, blink);
    paint_field(cx + 14, cy + 64, cw - 28, "Password", pass, field == 1, 1, blink);
    if (authfail)
        draw_text_ctr_t(W / 2, cy + 136, "Incorrect user or password, try again", rgb(C_ERROR), 1, 1);
    draw_text_ctr_t(W / 2, cy + 152, "user: root   password: rux", rgb(C_TEXT_DIM), 1, 1);
    draw_text_ctr_t(W / 2, H - 24, "[type] text    [Tab] switch field    [Enter] next / log in",
                    rgb(C_TEXT_DIM), 1, 1);
    draw_text_t(8, 8, "udesk session", rgb(C_TEXT_DIM), 1, 1);
    flushfb();
}

static void paint_window(int a) {
    const int x = WGE[a].x, y = WGE[a].y, w = WGE[a].w, h = WGE[a].h;
    int act = (focus == a);
    uint32_t tb = act ? rgb(C_TITLE_A) : rgb(C_TITLE_I);
    uint32_t acc = act ? rgb(C_ORANGE) : rgb(C_AUB_MID);
    rect(x + 4, y + h, w, 4, rgb(0x120006)); // drop shadow
    rect(x + w, y + 4, 4, h, rgb(0x120006));
    rect(x, y, w, TITLE_H, tb);
    hline(x, y + TITLE_H, w, acc);
    draw_text(x + 8, y + 4, WGE[a].title, acc, tb, 1, 2);
    rect(x + w - 18, y + 4, 14, 14, acc);
    draw_text(x + w - 15, y + 5, "x", rgb(C_AUB_DEEP), acc, 1, 1);
    rect(x, y + TITLE_H, w, h - TITLE_H, rgb(C_TERM_BG));
    if (a == APP_TERM) {
        int ty = y + TITLE_H + 6;
        for (int r = 0; r < TROWS; r++) {
            int tx = x + 8;
            for (int c = 0; c < TCOLS; c++) {
                uint8_t ch = tgrid[r][c];
                uint32_t fg = (ch == '$' || ch == '#') ? rgb(C_GREEN) : rgb(C_TERM_FG);
                tx += draw_char(tx, ty, ch, fg, rgb(C_TERM_BG), 1, 2) + 1;
            }
            ty += 17;
        }
        if (act && blink) rect(x + 8 + tcx * 9, y + TITLE_H + 6 + tcy * 17, 8, 16, rgb(C_GREEN));
    } else if (a == APP_INFO) {
        struct { const char *k; const char *v; } rows[] = {
            {"OS", "Ubuntu 22.04 LTS"}, {"Kernel", kver}, {"Host", "rux-ubuntu"},
            {"Uptime", upstr}, {"Memory", memstr}, {"Display", "fb0 640x480x32"},
            {"CPU", "RISC-V rv64 (QEMU virt)"}, {"Session", "udesk (native fb)"},
        };
        int ty = y + TITLE_H + 12;
        for (unsigned i = 0; i < sizeof rows / sizeof rows[0]; i++) {
            draw_text(x + 14, ty, rows[i].k, rgb(C_ORANGE), rgb(C_TERM_BG), 1, 2);
            draw_text(x + 110, ty, rows[i].v, rgb(C_TERM_FG), rgb(C_TERM_BG), 1, 2);
            ty += 22;
        }
        draw_text(x + 14, y + h - 16, "^R refresh", rgb(C_TEXT_DIM), rgb(C_TERM_BG), 1, 1);
    } else {
        draw_ubuntu_logo(x + w / 2, y + TITLE_H + 34, 22, rgb(C_ORANGE));
        draw_text_ctr(x + w / 2, y + TITLE_H + 66, "Ubuntu", rgb(C_WHITE), rgb(C_TERM_BG), 2, 2);
        draw_text_ctr(x + w / 2, y + TITLE_H + 90, "22.04 LTS (Jammy Jellyfish)", rgb(C_TERM_FG), rgb(C_TERM_BG), 1, 1);
        draw_text_ctr(x + w / 2, y + TITLE_H + 108, "native framebuffer session", rgb(C_TEXT_DIM), rgb(C_TERM_BG), 1, 1);
        draw_text_ctr(x + w / 2, y + TITLE_H + 124, "Rux OS kernel - RISC-V 64", rgb(C_TEXT_DIM), rgb(C_TERM_BG), 1, 1);
        draw_text_ctr(x + w / 2, y + TITLE_H + 148, "^T term  ^S info  ^A about", rgb(C_ORANGE), rgb(C_TERM_BG), 1, 1);
        draw_text_ctr(x + w / 2, y + TITLE_H + 162, "Tab cycle  ^W close", rgb(C_ORANGE), rgb(C_TERM_BG), 1, 1);
    }
}

static void paint_desktop(void) {
    paint_bg(PANEL_H, H);
    rect(0, 0, W, PANEL_H, rgb(C_AUB_DARK)); // top panel
    hline(0, PANEL_H - 1, W, rgb(C_AUB_MID));
    draw_ubuntu_logo(18, PANEL_H / 2, 9, rgb(C_ORANGE));
    draw_text(36, 6, "Ubuntu", rgb(C_WHITE), rgb(C_AUB_DARK), 1, 2);
    char clock[16];
    snprintf(clock, sizeof clock, "%02u:%02u:%02u", ticks / 3600, ticks / 60 % 60, ticks % 60);
    draw_text_ctr(W / 2, 6, clock, rgb(C_WHITE), rgb(C_AUB_DARK), 1, 2);
    rect(W - 70, 9, 10, 10, rgb(C_ORANGE));
    rect(W - 54, 9, 10, 10, rgb(C_GREEN));
    rect(W - 38, 9, 10, 10, rgb(C_WHITE));
    rect(0, PANEL_H, DOCK_W, H - PANEL_H, rgb(C_AUB_DEEP)); // dock
    vline(DOCK_W - 1, PANEL_H, H - PANEL_H, rgb(C_AUB_MID));
    const char *names[NAPPS] = {"Term", "Info", "About"};
    for (int a = 0; a < NAPPS; a++) {
        int by = PANEL_H + 20 + a * 64;
        uint32_t bd = win_open[a] ? (focus == a ? rgb(C_ORANGE) : rgb(C_AUB_MID)) : rgb(C_DOCK_BTN);
        frame(6, by, 44, 44, bd);
        draw_text_ctr(DOCK_W / 2, by + 46, names[a], win_open[a] ? rgb(C_WHITE) : rgb(C_TEXT_DIM),
                      rgb(C_AUB_DEEP), 1, 1);
        if (a == APP_TERM) draw_text_t(14, by + 8, ">_", rgb(C_GREEN), 2, 2);
        else if (a == APP_INFO) draw_char_t(20, by + 12, 'i', rgb(C_ORANGE), 2, 2);
        else draw_ubuntu_logo(28, by + 22, 12, rgb(C_ORANGE));
    }
    draw_text_ctr_t((W + DOCK_W) / 2, H - 16,
                    "^T terminal  ^S sysinfo  ^A about  Tab cycle  ^W close  (terminal takes plain keys)",
                    rgb(C_TEXT_DIM), 1, 1);
    int order[NAPPS] = {APP_TERM, APP_INFO, APP_ABOUT}; // focused paints last (z-order)
    for (int i = 0; i < NAPPS; i++) if (order[i] == focus) { order[i] = order[NAPPS - 1]; order[NAPPS - 1] = focus; }
    for (int i = 0; i < NAPPS; i++) if (win_open[order[i]]) paint_window(order[i]);
    flushfb();
}

// ------------------------------------------------------------------- plumbing
static int shell_in; // write end -> shell stdin

static void helper_key(int evw) {
    uint8_t m[3] = {EV_KEY, 1, 0};
    for (;;) {
        ssize_t n = read(0, m + 2, 1);
        if (n != 1) _exit(0);
        if (write(evw, m, 3) != 3) _exit(0);
    }
}
static void helper_tick(int evw) {
    uint8_t m[2] = {EV_TICK, 0}; // framed like every other event: type+len+data
    for (;;) { sleep(1); if (write(evw, m, 2) != 2) _exit(0); }
}
static void helper_shell(int out_r, int evw) {
    uint8_t m[EV_MAX];
    for (;;) {
        ssize_t n = read(out_r, m + 2, 200);
        if (n <= 0) _exit(0);
        m[0] = EV_SHELL; m[1] = (uint8_t)n;
        if (write(evw, m, 2 + n) != 2 + n) _exit(0);
    }
}

// framed reader with sticky buffer (frames never span reads un-parsed).
// NOTE: frame data is copied out to a scratch buffer BEFORE the sticky
// buffer is compacted — handing out rbuf+2 would alias the next frame's
// bytes after the memmove.
static uint8_t rbuf[2 * EV_MAX]; static int rlen; static uint8_t fdata[EV_MAX];
static int ev_next(uint8_t *type, const uint8_t **data, uint8_t *len, int fd) {
    for (;;) {
        if (rlen >= 2 && rbuf[1] <= EV_MAX - 2 && rlen >= 2 + rbuf[1]) {
            *type = rbuf[0]; *len = rbuf[1];
            memcpy(fdata, rbuf + 2, *len);
            *data = fdata;
            int used = 2 + *len;
            memmove(rbuf, rbuf + used, (size_t)(rlen - used));
            rlen -= used;
            return 1;
        }
        if (rlen >= 2 && rbuf[1] > EV_MAX - 2) { rlen = 0; continue; } // corrupt, resync
        ssize_t n = read(fd, rbuf + rlen, sizeof rbuf - rlen);
        if (n <= 0) continue;
        rlen += (int)n;
    }
}

// console raw mode: kernel-ABI termios = 4 tcflag_t + c_line + c_cc[19] = 36 bytes
// c_oflag=OPOST|ONLCR keeps kernel printf sane; lflag=0 drops ICANON/ECHO/ISIG.
static void console_raw(void) {
    uint8_t t[36];
    memset(t, 0, sizeof t);
    t[4] = 5;   // c_oflag = OPOST|ONLCR
    t[23] = 1;  // c_cc[VMIN] = 1
    ioctl(0, TCSETS, t);
}

static void open_app(int a) {
    win_open[a] = 1; focus = a;
    printf("[udesk] app open+focus: %d\n", a);
}
static void cycle_focus(void) {
    int a = focus;
    for (int i = 1; i <= NAPPS; i++) {
        int c = (focus + i) % NAPPS;
        if (win_open[c]) { a = c; break; }
    }
    focus = a;
    printf("[udesk] focus -> %d\n", focus);
}

static void handle_key(uint8_t k) {
    if (state_login) {
        if (k == '\t') { field ^= 1; printf("[udesk] login field -> %s\n", field ? "password" : "username"); }
        else if (k == '\n' || k == '\r') {
            if (field == 0) { field = 1; printf("[udesk] login user='%s' -> password\n", user); }
            else if (!strcmp(user, "root") && !strcmp(pass, "rux")) {
                state_login = 0;
                printf("[udesk] LOGIN OK -> desktop\n");
                open_app(APP_TERM);
            } else {
                authfail = 1; plen = 0; pass[0] = 0;
                printf("[udesk] LOGIN FAIL (user='%s')\n", user);
            }
        }
        else if (k == 0x7f || k == 0x08) {
            if (field == 0 && ulen) user[--ulen] = 0;
            if (field == 1 && plen) pass[--plen] = 0;
        }
        else if (k >= 32 && k < 127) {
            if (field == 0 && ulen < 38) { user[ulen++] = (char)k; user[ulen] = 0; }
            else if (field == 1 && plen < 38) { pass[plen++] = (char)k; pass[plen] = 0; }
        }
        return;
    }
    if (k == 0x14) open_app(APP_TERM);                        // Ctrl-T
    else if (k == 0x13) { open_app(APP_INFO); sysinfo_refresh(); } // Ctrl-S
    else if (k == 0x01) open_app(APP_ABOUT);                  // Ctrl-A
    else if (k == '\t') cycle_focus();
    else if (k == 0x17 || k == 0x1b) { win_open[focus] = 0; printf("[udesk] close win %d\n", focus); cycle_focus(); }
    else if (k == 0x12) { sysinfo_refresh(); focus = APP_INFO; win_open[APP_INFO] = 1; printf("[udesk] sysinfo refresh\n"); }
    else if (focus == APP_TERM && win_open[APP_TERM]) {
        uint8_t c = (k == '\r') ? '\n' : k;
        if (write(shell_in, &c, 1) == 1) printf("[udesk] term key 0x%02x\n", k);
    }
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    fbfd = open("/dev/fb0", O_RDWR);
    if (fbfd < 0) { printf("[udesk] no /dev/fb0\n"); return 1; }
    struct fb_var v; memset(&v, 0, sizeof v);
    ioctl(fbfd, FBIOGET_VSCREENINFO, &v);
    W = v.xres ? (int)v.xres : 640; H = v.yres ? (int)v.yres : 480;
    fb = mmap(0, (size_t)W * H * 4, PROT_READ | PROT_WRITE, MAP_SHARED, fbfd, 0);
    if (fb == MAP_FAILED) { printf("[udesk] mmap fail\n"); return 1; }
    printf("[udesk] fb %dx%d\n", W, H);
    init_wge();
    console_raw();
    sysinfo_refresh();
    memset(tgrid, ' ', sizeof tgrid);

    // event bus
    int ev[2];
    if (pipe(ev)) { printf("[udesk] ev pipe fail\n"); return 1; }

    // shell for the Terminal app (pipe pair — pty slave ENXIO is a known gap)
    int in_p[2], out_p[2];
    if (pipe(in_p) || pipe(out_p)) { printf("[udesk] pipe fail\n"); return 1; }
    pid_t pid = fork();
    if (pid == 0) {
        setsid();
        dup2(in_p[0], 0); dup2(out_p[1], 1); dup2(out_p[1], 2);
        char *av[] = {"/bin/dash", NULL};
        char *evn[] = {"HOME=/root", "PATH=/bin:/usr/bin:/sbin", "TERM=dumb", "PS1=# ", NULL};
        execve(av[0], av, evn);
        _exit(127);
    }
    close(in_p[0]); close(out_p[1]);
    shell_in = in_p[1];
    if (fork() == 0) { close(ev[0]); close(in_p[1]); helper_shell(out_p[0], ev[1]); _exit(0); }
    if (fork() == 0) { close(ev[0]); helper_key(ev[1]); _exit(0); }
    if (fork() == 0) { close(ev[0]); helper_tick(ev[1]); _exit(0); }
    close(ev[1]);

    paint_login();
    printf("[udesk] login screen up\n");

    uint8_t type, len; const uint8_t *data;
    for (;;) {
        if (!ev_next(&type, &data, &len, ev[0])) continue;
        if (type == EV_KEY) {
            uint8_t k = data[0];
            printf("[udesk] key 0x%02x '%c'\n", k, k >= 32 && k < 127 ? (char)k : '.');
            handle_key(k);
            if (state_login) paint_login(); else paint_desktop();
        } else if (type == EV_TICK) {
            ticks++; blink = !blink;
            if (state_login) paint_login(); else paint_desktop();
        } else if (type == EV_SHELL) {
            (void)!write(2, data, len); // mirror shell output to the serial log (verification aid)
            for (int i = 0; i < len; i++) term_putc(data[i]);
            if (!state_login && win_open[APP_TERM]) paint_desktop();
        }
    }
}
