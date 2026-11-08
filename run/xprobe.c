/* xprobe — minimal X-protocol client: connection setup + one request.
 *
 * 1. connect to /tmp/.X11-unix/X0 (the X server's unix socket)
 * 2. send the 12-byte connection-setup header (byte order 'l',
 *    protocol 11, no auth) and read the server's reply
 * 3. round-trip a GetInputFocus request and verify the reply
 *
 * Verdicts:
 *   connect fail     -> the server is not accepting (wedge before socket)
 *   setup fail       -> the server cannot complete a handshake
 *   request fail     -> the server accepts but never answers requests
 *   all ok           -> X is alive at the protocol level; a black screen
 *                       then means the RENDERING path (render/glx/present)
 *                       is the wedge.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

static int readn(int fd, void *b, size_t n)
{
    size_t g = 0;
    while (g < n) {
        ssize_t r = read(fd, (char *)b + g, n - g);
        if (r < 0) { if (errno == EINTR) continue; return -1; }
        if (r == 0) return -2;
        g += r;
    }
    return 0;
}

static int writen(int fd, const void *b, size_t n)
{
    size_t p = 0;
    while (p < n) {
        ssize_t w = write(fd, (const char *)b + p, n - p);
        if (w < 0) { if (errno == EINTR) continue; return -1; }
        p += w;
    }
    return 0;
}

int main(int argc, char **argv)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    const char *path = argc > 1 ? argv[1] : "/tmp/.X11-unix/X0";

    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un sa = { .sun_family = AF_UNIX };
    strncpy(sa.sun_path, path, sizeof sa.sun_path - 1);
    if (connect(fd, (struct sockaddr *)&sa, sizeof sa) < 0) {
        printf("XPROBE connect %s FAILED: %s\n", path, strerror(errno));
        return 1;
    }
    printf("XPROBE connected\n");

    /* Connection setup: CARD8 byte-order 'l', CARD8 pad, CARD16 major=11,
       CARD16 minor=0, CARD16 name-len=0, CARD16 data-len=0, CARD16 pad */
    unsigned char setup[12] = { 'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0 };
    if (writen(fd, setup, 12) != 0) { printf("XPROBE setup write failed\n"); return 2; }

    /* Reply: 1 byte success(1)/fail(0)/auth(2), 1 pad, CARD16 major,
       CARD16 extra, then (success) 24-byte body + vendor + padlock. */
    unsigned char hdr[8];
    if (readn(fd, hdr, 8) != 0) { printf("XPROBE setup NO REPLY\n"); return 3; }
    printf("XPROBE setup reply: success=%d major=%d extra=%d\n",
           hdr[0], hdr[2] | (hdr[3] << 8), hdr[6] | (hdr[7] << 8));
    if (hdr[0] != 1) { printf("XPROBE setup FAILED (not success)\n"); return 4; }

    unsigned int extra = hdr[6] | (hdr[7] << 8);
    /* read the rest of the fixed body (24) + extra*4 */
    unsigned char body[24 + 512];
    size_t want = 24 + (size_t)extra * 4;
    if (want > sizeof body) want = sizeof body;
    if (readn(fd, body, want) != 0) { printf("XPROBE setup body short\n"); return 5; }
    /* vendor string is at offset 0 of the additional data area: 16-byte name */
    char vendor[17] = { 0 };
    memcpy(vendor, body + 16, 16);
    printf("XPROBE vendor=%.16s\n", vendor);

    /* GetInputFocus (opcode 43), reply = 32 bytes */
    unsigned char req[4] = { 43, 0, 1, 0 };
    if (writen(fd, req, 4) != 0) { printf("XPROBE gif write failed\n"); return 6; }
    unsigned char rep[32];
    if (readn(fd, rep, 32) != 0) { printf("XPROBE GetInputFocus NO REPLY — server accepts but never answers\n"); return 7; }
    printf("XPROBE GetInputFocus reply ok (type=%d revert-to=%d)\n", rep[0], rep[1]);

    printf("XPROBE-DONE ok\n");
    return 0;
}
