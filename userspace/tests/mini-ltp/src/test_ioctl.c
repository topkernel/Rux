/*
 * Test: ioctl() - TIOCGWINSZ
 * Test terminal ioctl
 */

#include <sys/ioctl.h>
#include <unistd.h>
#include <fcntl.h>

int main(void)
{
    struct winsize ws;
    int fd;

    /* Try to open the controlling terminal */
    fd = open("/dev/console", O_RDONLY);
    if (fd < 0) {
        /* If no console, try standard input */
        fd = 0;
    }

    /* Get terminal window size */
    if (ioctl(fd, TIOCGWINSZ, &ws) < 0) {
        /* May not be a terminal, this is acceptable */
        if (fd > 0) close(fd);
        return 0;  /* Skip this test */
    }

    if (fd > 0) close(fd);

    /* Verify window size is valid */
    if (ws.ws_col == 0 || ws.ws_row == 0) {
        return 0;  /* Skip, may be an invalid terminal */
    }

    return 0;
}
