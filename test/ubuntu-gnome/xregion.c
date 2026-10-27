/* xregion.c — deterministic XGetImage probes (leg-7; -w walk leg-8).
 *
 * The window-painting frontier: the xterm probe window MAPS and gets
 * framed by mutter (xwininfo -tree shows 904x536+140+127 under a frame)
 * yet no pixel of it ever appears in the virtio-gpu scanout, while the
 * shell's own chrome (panel, wallpaper, cursor) paints fine. The open
 * question is WHERE the chain breaks:
 *
 *   client draws -> [window backing pixels] -> mutter composites (GLX
 *   TFP) -> [stage/root content] -> Xorg shadow-fb/damage -> [scanout]
 *
 * Modes:
 *   xregion X Y W H      rect of the root window (leg-7 semantics)
 *   xregion -w [minpx]   walk the whole window tree from root; for each
 *                        window print id/name/class/geometry/map-state
 *                        and, when >= minpx (default 100) on a side,
 *                        the same pixel stats via XGetImage on the
 *                        WINDOW ITSELF. That splits the frontier in one
 *                        dump: an xterm body that is white-with-glyphs
 *                        in its own window but absent from scanout
 *                        convicts the mutter composite path (TFP); an
 *                        all-black xterm window convicts client drawing
 *                        (no first Expose / XGC), independent of mutter.
 *
 * Dynamic against the image's own libX11 (runs long after the rootfs is
 * mounted). Sampled at stride 2 (rect) / 4 (walk) in both axes to keep
 * TCG cost sane; window bodies are large flat rectangles so sampling
 * cannot miss them.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <X11/Xlib.h>
#include <X11/Xutil.h>

/* Local decimal parse: the host cross-gcc's glibc headers redirect
 * atoi/strtol to __isoc23_* (glibc 2.38) while the image runs jammy's
 * 2.35 — the plain digit loop keeps the binary loadable there. */
static int xr_atoi(const char *s)
{
    int v = 0, neg = 0;
    while (*s == ' ' || *s == '\t')
        s++;
    if (*s == '-') { neg = 1; s++; }
    while (*s >= '0' && *s <= '9')
        v = v * 10 + (*s++ - '0');
    return neg ? -v : v;
}

/* Xlib default error handling _exit()s the process; we want a NULL
 * XGetImage (e.g. BadMatch on a window being reparented mid-walk) to
 * just skip that window. The handler also STASHES the error code /
 * request / resourceid: r12walk showed every sizable client window
 * (xterm probe + its mutter frame) returning getimage-failed while
 * root-window GetImage of a LARGER rect succeeds in the same process
 * to the same server — the error code is the next deterministic datum
 * (BadMatch=8 vs BadDrawable=9 vs garbled-reply land). GetImage is a
 * replied request: the handler has run by the time XGetImage returns
 * NULL; the XSync calls are belt-and-braces. */
static volatile int g_xerr_code, g_xerr_req, g_xerr_minor;
static volatile unsigned long g_xerr_res;

static int xr_xerr(Display *d, XErrorEvent *ev)
{
    (void)d;
    g_xerr_code = ev->error_code;
    g_xerr_req = ev->request_code;
    g_xerr_minor = ev->minor_code;
    g_xerr_res = (unsigned long)ev->resourceid;
    return 0;
}

/* Sample an XImage (already fetched) at stride 2 (rect mode) or 4
 * (walk mode, TCG cost) and print the one-line stats tail. */
static void xr_stats_tail(XImage *im, int stride)
{
    long n = 0, white = 0, black = 0;
    unsigned hist[4096];
    memset(hist, 0, sizeof hist);

    for (int y = 0; y < im->height; y += stride) {
        for (int x = 0; x < im->width; x += stride) {
            unsigned long px = XGetPixel(im, x, y);
            int r = (int)((px >> 16) & 0xff);
            int g = (int)((px >> 8) & 0xff);
            int b = (int)(px & 0xff);
            n++;
            if (r >= 0xe0 && g >= 0xe0 && b >= 0xe0)
                white++;
            if (r < 0x20 && g < 0x20 && b < 0x20)
                black++;
            hist[(((unsigned)r >> 4) << 8) | (((unsigned)g >> 4) << 4) |
                 ((unsigned)b >> 4)]++;
        }
    }
    if (!n) {
        printf("empty\n");
        return;
    }

    int topk[4] = { -1, -1, -1, -1 };
    unsigned topn[4] = { 0, 0, 0, 0 };
    for (int i = 0; i < 4096; i++) {
        if (!hist[i])
            continue;
        for (int s = 0; s < 4; s++) {
            if (hist[i] > topn[s]) {
                for (int m = 3; m > s; m--) {
                    topn[m] = topn[m - 1];
                    topk[m] = topk[m - 1];
                }
                topn[s] = hist[i];
                topk[s] = i;
                break;
            }
        }
    }

    printf("sample=%ld white=%ld(%.1f%%) black=%ld(%.1f%%) top=",
           n, white, 100.0 * white / n,
           black, 100.0 * black / n);
    for (int s = 0; s < 4; s++) {
        if (!topn[s])
            break;
        unsigned k = (unsigned)topk[s];
        printf("%s#%x%x%x:%.1f%%", s ? "," : "",
               (k >> 8) & 0xf, (k >> 4) & 0xf, k & 0xf,
               100.0 * topn[s] / n);
    }
    printf("\n");
}

/* XGetImage up to `cap` pixels of a window's own content and print the
 * stats. Content of a redirected (composited) window is its off-screen
 * backing store — exactly the pixels mutter would texture from. */
static void xr_window_content(Display *d, Window w, int ww, int wh)
{
    int cw = ww > 480 ? 480 : ww;
    int ch = wh > 320 ? 320 : wh;
    g_xerr_code = 0; g_xerr_req = 0; g_xerr_minor = 0; g_xerr_res = 0;
    XSync(d, False); /* drain any stale error */
    XImage *im = XGetImage(d, w, 0, 0, (unsigned)cw, (unsigned)ch,
                           AllPlanes, ZPixmap);
    if (!im) {
        XSync(d, False);
        printf("content=getimage-failed err=%d req=%d minor=%d res=0x%lx",
               g_xerr_code, g_xerr_req, g_xerr_minor, g_xerr_res);
        /* one retry after a beat: a transient BadMatch on a window
         * being reparented mid-walk must not read as a hard failure */
        usleep(200000);
        g_xerr_code = 0;
        im = XGetImage(d, w, 0, 0, (unsigned)cw, (unsigned)ch,
                       AllPlanes, ZPixmap);
        if (!im) {
            XSync(d, False);
            printf(" retry-err=%d req=%d res=0x%lx\n",
                   g_xerr_code, g_xerr_req, g_xerr_res);
            return;
        }
        printf(" retry-ok ");
    }
    xr_stats_tail(im, 4);
    XDestroyImage(im);
}

static int g_walked;

/* Depth-first walk from `w`, printing every window and sampling the
 * sizable ones. Bounded: 256 windows, depth 8. */
static void xr_walk(Display *d, Window w, int depth, int minpx)
{
    if (depth > 8 || g_walked > 256)
        return;
    g_walked++;

    char *name = NULL;
    XClassHint ch = { NULL, NULL };
    XWindowAttributes wa;
    int have_wa = XGetWindowAttributes(d, w, &wa) != 0;
    XFetchName(d, w, &name);
    XGetClassHint(d, w, &ch);

    unsigned ww = have_wa ? (unsigned)wa.width : 0;
    unsigned wh = have_wa ? (unsigned)wa.height : 0;
    int mapstate = have_wa ? wa.map_state : -1;

    printf("XWIN 0x%lx depth=%d map=%d %ux%u name=\"%s\" class=\"%s\"",
           (unsigned long)w, have_wa ? wa.depth : -1, mapstate, ww, wh,
           name ? name : "", ch.res_class ? ch.res_class : "");
    if (name) XFree(name);
    if (ch.res_name) XFree(ch.res_name);
    if (ch.res_class) XFree(ch.res_class);

    if (have_wa && wa.depth > 0 && (int)ww >= minpx && (int)wh >= minpx) {
        printf(" -> ");
        fflush(stdout);
        xr_window_content(d, w, (int)ww, (int)wh);
    } else {
        printf("\n");
    }

    Window rroot, parent, *kids = NULL;
    unsigned nkids = 0;
    if (XQueryTree(d, w, &rroot, &parent, &kids, &nkids) && kids) {
        /* sample children last-to-first so a client window is described
         * right after its frame in the log */
        for (int i = (int)nkids - 1; i >= 0; i--)
            xr_walk(d, kids[i], depth + 1, minpx);
        XFree(kids);
    }
}

int main(int argc, char **argv)
{
    Display *d = XOpenDisplay(NULL);
    if (!d) {
        printf("XREGION no-display\n");
        return 1;
    }
    XSetErrorHandler(xr_xerr);
    Window root = DefaultRootWindow(d);
    int sw = DisplayWidth(d, DefaultScreen(d));
    int sh = DisplayHeight(d, DefaultScreen(d));

    if (argc >= 2 && strcmp(argv[1], "-w") == 0) {
        int minpx = argc >= 3 ? xr_atoi(argv[2]) : 100;
        g_walked = 0;
        printf("XWALK screen=%dx%d minpx=%d\n", sw, sh, minpx);
        fflush(stdout);
        xr_walk(d, root, 0, minpx);
        printf("XWALK done windows=%d\n", g_walked);
        XCloseDisplay(d);
        return 0;
    }

    if (argc < 5) {
        fprintf(stderr, "usage: xregion X Y W H | xregion -w [minpx]\n");
        return 2;
    }
    int rx = xr_atoi(argv[1]), ry = xr_atoi(argv[2]);
    int rw = xr_atoi(argv[3]), rh = xr_atoi(argv[4]);

    if (rx < 0) rx = 0;
    if (ry < 0) ry = 0;
    if (rx + rw > sw) rw = sw - rx;
    if (ry + rh > sh) rh = sh - ry;
    if (rw <= 0 || rh <= 0) {
        printf("XREGION degenerate %d,%d %dx%d (screen %dx%d)\n",
               rx, ry, rw, rh, sw, sh);
        return 1;
    }

    XImage *im = XGetImage(d, root, rx, ry, (unsigned)rw, (unsigned)rh,
                           AllPlanes, ZPixmap);
    if (!im) {
        printf("XREGION getimage-failed %d,%d %dx%d\n", rx, ry, rw, rh);
        return 1;
    }

    printf("XREGION %d,%d %dx%d ", rx, ry, rw, rh);
    fflush(stdout);
    xr_stats_tail(im, 2);
    XDestroyImage(im);
    XCloseDisplay(d);
    return 0;
}
