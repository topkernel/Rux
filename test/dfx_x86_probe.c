/* dfx_x86_probe.c — x86_64 DFX smoke probe.
 *
 * Boots as init (init=/test/dfx_x86_probe) and dumps the DFX procfs
 * surfaces to the serial console between markers, so the shapes can be
 * compared against a Linux host without needing a multi-call shell.
 * Static build: gcc -static -O1.
 */
#include <fcntl.h>
#include <unistd.h>
#include <string.h>
#include <sys/reboot.h>

static void dump(const char *path)
{
    char buf[2048];
    ssize_t n;
    int fd;

    write(1, "===XSMOKE: ", 11);
    write(1, path, strlen(path));
    write(1, "\n", 1);
    fd = open(path, O_RDONLY);
    if (fd < 0) {
        write(1, "(open failed)\n", 14);
        return;
    }
    while ((n = read(fd, buf, sizeof(buf))) > 0)
        write(1, buf, (size_t)n);
    close(fd);
}

int main(void)
{
    write(1, "===XSMOKE: start\n", 17);
    dump("/proc/stat");
    dump("/proc/vmstat");
    dump("/proc/schedstat");
    dump("/proc/softirqs");
    dump("/proc/buddyinfo");
    dump("/proc/sys/kernel/tainted");
    dump("/proc/locks");
    dump("/proc/kallsyms");
    write(1, "===XSMOKE: done\n", 16);
    /* Give the serial console a moment, then power off politely. */
    sleep(1);
    reboot(0x4321fedc); /* LINUX_REBOOT_CMD_POWER_OFF */
    return 0;
}
