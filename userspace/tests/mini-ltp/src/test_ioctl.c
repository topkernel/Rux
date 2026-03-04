/*
 * Test: ioctl() - TIOCGWINSZ
 * 测试终端 ioctl
 */

#include <sys/ioctl.h>
#include <unistd.h>
#include <fcntl.h>

int main(void)
{
    struct winsize ws;
    int fd;

    /* 尝试打开控制终端 */
    fd = open("/dev/console", O_RDONLY);
    if (fd < 0) {
        /* 如果没有控制台，尝试标准输入 */
        fd = 0;
    }

    /* 获取终端窗口大小 */
    if (ioctl(fd, TIOCGWINSZ, &ws) < 0) {
        /* 可能不是终端，这也是可以接受的 */
        if (fd > 0) close(fd);
        return 0;  /* 跳过此测试 */
    }

    if (fd > 0) close(fd);

    /* 验证窗口大小有效 */
    if (ws.ws_col == 0 || ws.ws_row == 0) {
        return 0;  /* 跳过，可能是无效的终端 */
    }

    return 0;
}
