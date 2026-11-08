/* xfill — hand-rolled X protocol: create a white full-screen window.
 *
 * If this paints (host screendump goes non-black), the X server's
 * render + shadow->framebuffer copy chain is INTACT and the black
 * screen belongs to mutter's frame production; if it stays black,
 * the X server side (shadow refresh / fb write) is the wedge.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

static int fd;
static unsigned char buf[256];

static void xreq(const void *b, size_t n)
{
    const unsigned char *p = b;
    size_t put = 0;
    while (put < n) {
        ssize_t w = write(fd, p + put, n - put);
        if (w < 0 && errno == EINTR) continue;
        if (w < 0) { printf("XFILL write failed\n"); return; }
        put += w;
    }
}

static int readn(void *b, size_t n)
{
    size_t g = 0;
    while (g < n) {
        ssize_t r = read(fd, (char *)b + g, n - g);
        if (r < 0 && errno == EINTR) continue;
        if (r <= 0) return -1;
        g += r;
    }
    return 0;
}

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("XFILL start\n");
    fd = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un sa = { .sun_family = AF_UNIX };
    strcpy(sa.sun_path, "/tmp/.X11-unix/X0");
    if (connect(fd, (struct sockaddr *)&sa, sizeof sa) < 0) {
        printf("XFILL connect failed: %s\n", strerror(errno));
        return 1;
    }
    /* setup */
    unsigned char setup[12] = { 'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0 };
    xreq(setup, 12);
    unsigned char hdr[8];
    if (readn(hdr, 8) || hdr[0] != 1) { printf("XFILL setup failed\n"); return 2; }
    unsigned extra = hdr[6] | (hdr[7] << 8);
    static unsigned char body[16384];
    if (readn(body, 24 + (size_t)extra * 4)) { printf("XFILL body short\n"); return 3; }
    unsigned ridBase = body[4] | (body[5] << 8) | (body[6] << 16) | (body[7] << 24);
    unsigned numFormats = body[23];
    unsigned root = *(unsigned *)(body + 24 + 8 * numFormats);
    unsigned sw = *(unsigned short *)(body + 24 + 8 * numFormats + 40);
    unsigned sh = *(unsigned short *)(body + 24 + 8 * numFormats + 42);
    printf("XFILL root=%#x screen=%ux%u ridBase=%#x\n", root, sw, sh, ridBase);

    unsigned wid = ridBase | 0x00200000;
    unsigned gc = ridBase | 0x00200001;

    /* CreateWindow: value-mask bit 1 = background-pixel (white) */
    {
        unsigned r[16] = { 0 };
        unsigned short *h = (unsigned short *)r;
        r[0] = 0;                 /* opcode(1)=1 filled below */
        ((unsigned char *)r)[0] = 1;
        h[1] = 10;                /* length 10 words */
        r[1] = wid;               /* wid */
        r[2] = root;              /* parent */
        h[4] = 0; h[5] = 0;       /* x, y */
        h[6] = sw; h[7] = sh;     /* w, h */
        h[8] = 0;                 /* border width */
        h[9] = 1;                 /* class InputOutput */
        r[5] = 0;                 /* visual CopyFromParent */
        r[6] = 1;                 /* mask: background-pixel */
        r[7] = 0x00FFFFFF;        /* white */
        r[8] = 2;                 /* mask: border-pixel */
        r[9] = 0x00FF0000;        /* red border, unmistakable */
        /* recount: words: 4(hdr)+wid+parent+5*2(x,y,w,h,bw)+class+pad? */
        xreq(r, 10 * 4);
    }
    /* CreateGC with foreground red */
    {
        unsigned r[8] = { 0 };
        ((unsigned char *)r)[0] = 55;
        ((unsigned short *)r)[1] = 6;   /* length 6 words */
        r[1] = gc;
        r[2] = wid;
        r[3] = 4;              /* mask: foreground */
        r[4] = 0x00FF0000;     /* red */
        xreq(r, 6 * 4);
    }
    /* MapWindow */
    {
        unsigned r[2] = { 0 };
        ((unsigned char *)r)[0] = 8;
        ((unsigned short *)r)[1] = 2;
        r[1] = wid;
        xreq(r, 8);
    }
    /* PolyFillRectangle: half the screen red */
    {
        unsigned r[8] = { 0 };
        unsigned short *h = (unsigned short *)r;
        ((unsigned char *)r)[0] = 69;
        h[1] = 7;
        r[1] = wid;
        r[2] = gc;
        h[6] = 100;
        h[7] = 100;
        h[8] = sw / 2;
        h[9] = sh - 200;
        xreq(r, 7 * 4);
    }
    printf("XFILL requests sent (window+gc+map+fill)\n");

    /* round-trip GetInputFocus to force a flush, then idle */
    unsigned gif[1] = { 0 };
    ((unsigned char *)gif)[0] = 43;
    ((unsigned short *)gif)[1] = 2;
    xreq(gif, 4);
    unsigned char rep[32];
    if (readn(rep, 32)) { printf("XFILL flush read failed\n"); return 4; }
    printf("XFILL flushed; window should be visible now\n");
    sleep(25);
    printf("XFILL-DONE\n");
    return 0;
}
