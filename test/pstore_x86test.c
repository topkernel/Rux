/* pstore_x86test.c — x86_64 pstore anchor cycle probe (as init).
 *
 * One binary, two boots (a guest reset does not clear RAM, so the
 * reserved region keeps the last panic's record):
 *
 *   boot 1: /sys/fs/pstore/console-ramoops-0 unreadable -> print the
 *           boot-1 marker, write 'c' into /proc/sysrq-trigger (panic;
 *           the kernel cmdline carries panic=3 so the guest reboots).
 *
 *   boot 2: the file IS readable -> print the boot-2 marker and the
 *           record's head, then idle (the driver greps the serial).
 *
 * Built static (the pbench toolchain) and staged as /test/px by
 * test/pstore-x86-smoke.sh.
 */
#include <fcntl.h>
#include <unistd.h>

int main(void)
{
    static const char rec[] = "/sys/fs/pstore/console-ramoops-0";
    int fd = open(rec, O_RDONLY);
    if (fd >= 0) {
        char buf[1024];
        ssize_t n = read(fd, buf, sizeof(buf) - 1);
        close(fd);
        if (n > 0) {
            buf[n] = '\0';
            write(1, "===PX2: record resurfaced===\n", 29);
            write(1, buf, (size_t)n);
            write(1, "===PX2: record end===\n", 22);
            for (;;)
                pause();
        }
    }
    write(1, "===PX1: crashing via sysrq c===\n", 32);
    fd = open("/proc/sysrq-trigger", O_WRONLY);
    if (fd < 0) {
        write(1, "===PX1: no sysrq-trigger===\n", 28);
        return 1;
    }
    if (write(fd, "c", 1) != 1) {
        write(1, "===PX1: sysrq write failed===\n", 30);
        return 1;
    }
    for (;;)
        pause();
    return 0;
}
