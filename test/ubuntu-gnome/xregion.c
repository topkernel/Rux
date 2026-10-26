/* xregion.c — deterministic XGetImage region probe (leg-7).
 *
 * The window-painting frontier: the xterm probe window MAPS and gets
 * framed by mutter (xwininfo -tree shows 904x536+140+127 under a frame)
 * yet no pixel of it ever appears in the virtio-gpu scanout, while the
 * shell's own chrome (panel, wallpaper, cursor) paints fine. The open
 * question is WHERE the chain breaks:
 *
 *   client draws -> [X pixmap] -> mutter composites (GLX TFP) ->
 *   [root window content] -> Xorg shadow-fb/damage -> [scanout]
 *
 * This probe XGetImage's a rectangle of the ROOT window and prints a
 * one-line deterministic summary (pixel counts + quantized top colors)
 * to stdout. Compare against the QMP screendump of the same region
 * (fire.py shots + run/panelcheck.py):
 *   - root region has the window, scanout does not  -> display path
 *   - root region lacks the window too              -> mutter compositing
 *
 * Dynamic against the image's own libX11 (runs long after the rootfs is
 * mounted). Sampled at stride 2 in both axes to keep TCG cost sane;
 * the window body is a large flat rectangle so sampling cannot miss it.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <X11/Xlib.h>
#include <X11/Xutil.h>

int main(int argc, char **argv)
{
    if (argc < 5) {
        fprintf(stderr, "usage: xregion X Y W H\n");
        return 2;
    }
    int rx = atoi(argv[1]), ry = atoi(argv[2]);
    int rw = atoi(argv[3]), rh = atoi(argv[4]);

    Display *d = XOpenDisplay(NULL);
    if (!d) {
        printf("XREGION no-display\n");
        return 1;
    }
    Window root = DefaultRootWindow(d);
    int sw = DisplayWidth(d, DefaultScreen(d));
    int sh = DisplayHeight(d, DefaultScreen(d));
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

    long n = 0, white = 0, black = 0;
    unsigned hist[4096];
    memset(hist, 0, sizeof hist);

    for (int y = 0; y < rh; y += 2) {
        for (int x = 0; x < rw; x += 2) {
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

    /* top-4 quantized colors, insertion into a 4-slot max list */
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

    printf("XREGION %d,%d %dx%d sample=%ld white=%ld(%.1f%%) "
           "black=%ld(%.1f%%) top=",
           rx, ry, rw, rh, n,
           white, n ? 100.0 * white / n : 0.0,
           black, n ? 100.0 * black / n : 0.0);
    for (int s = 0; s < 4; s++) {
        if (!topn[s])
            break;
        unsigned k = (unsigned)topk[s];
        printf("%s#%x%x%x:%.1f%%", s ? "," : "",
               (k >> 8) & 0xf, (k >> 4) & 0xf, k & 0xf,
               100.0 * topn[s] / n);
    }
    printf("\n");
    XDestroyImage(im);
    XCloseDisplay(d);
    return 0;
}
