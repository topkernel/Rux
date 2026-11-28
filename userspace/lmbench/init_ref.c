/* init_ref.c — PID-1 wrapper for the reference-kernel column of the
 * lmbench rig (run-refkvm.sh).
 *
 * The reference kernel (the host's generic distro kernel, same binary
 * the native reference column runs on) boots the same ext4 rootfs as
 * the Rux column with init=/test/init_ref. Unlike Rux, it does not
 * mount /proc for PID 1, and the Ubuntu rootfs carries no static
 * /dev/console — so this wrapper prepares the minimal environment
 * lmbench_run expects and then execs it (PID 1 is preserved: the
 * runner's reboot(RB_POWER_OFF) still ends the boot):
 *
 *   - mount devtmpfs/proc/sysfs
 *   - open /dev/console and dup2 it onto 0/1/2 (the kernel could not
 *     open an initial console from the empty on-disk /dev; serial
 *     output only reaches the host once this is done)
 *   - exec /test/lmbench_run (same binary the Rux column uses)
 *
 * The guest /tmp is the ext4 /tmp; build-rootfs-ref.sh places the
 * static /tmp/hello binary there so lat_proc's exec case measures a
 * real execve, exactly like the native reference column.
 */
#include <fcntl.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/socket.h>
#include <sys/ioctl.h>
#include <net/if.h>
#include <string.h>
#include <unistd.h>

int main(void)
{
    mkdir("/dev", 0755);
    mkdir("/proc", 0755);
    mkdir("/sys", 0755);
    mkdir("/tmp", 0755);

    mount("devtmpfs", "/dev", "devtmpfs", 0, 0);
    mount("proc", "/proc", "proc", 0, 0);
    mount("sysfs", "/sys", "sysfs", 0, 0);

    int c = open("/dev/console", O_RDWR);
    if (c >= 0) {
        dup2(c, 0);
        dup2(c, 1);
        dup2(c, 2);
        if (c > 2)
            close(c);
    }
    /* Bring up the loopback interface: the reference kernel does not
     * auto-configure it, and lat_tcp/lat_udp/lat_connect target
     * 127.0.0.1 — without SIOCSIFFLAGS(IFF_UP) connect(2) fails with
     * ENETUNREACH. Same one-time setup ifconfig lo up performs. */
    {
        struct ifreq ifr;
        int s = socket(AF_INET, SOCK_DGRAM, 0);
        if (s >= 0) {
            memset(&ifr, 0, sizeof(ifr));
            strncpy(ifr.ifr_name, "lo", IFNAMSIZ);
            ioctl(s, SIOCGIFFLAGS, &ifr);
            ifr.ifr_flags |= IFF_UP | IFF_RUNNING;
            ioctl(s, SIOCSIFFLAGS, &ifr);
            close(s);
        }
    }

    write(1, "REFKVM INIT WRAPPER OK\n", 23);

    execl("/test/lmbench_run", "lmbench_run", (char *)0);

    /* exec failed — park; the outer timeout reaps the boot */
    for (;;)
        pause();
}
