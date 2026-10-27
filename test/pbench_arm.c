/* pbench_arm.c — A-side wrapper for the schedstat cost A-B (T7 slice):
 * write 1 into /proc/sys/kernel/sched_schedstats, print the read-back
 * (the serial log then carries the armed proof next to the PBENCH
 * lines), then execve the probe unchanged. Built static by
 * pbench_boot.py --arm-schedstat and staged as /test/arm; the boot's
 * init becomes /test/arm instead of /test/pbench, so the B side is the
 * stock pbench boot of the same kernel, same image recipe.
 */
#include <fcntl.h>
#include <unistd.h>

int main(void)
{
    static const char path[] = "/proc/sys/kernel/sched_schedstats";
    int fd = open(path, O_WRONLY);
    if (fd >= 0) {
        if (write(fd, "1", 1) == 1)
            write(1, "[arm] sched_schedstats=1\n", 25);
        close(fd);
    } else {
        write(1, "[arm] open sched_schedstats failed\n", 35);
    }
    char *argv[] = { "/test/pbench", 0 };
    char *envp[] = { 0 };
    execve(argv[0], argv, envp);
    return 127; /* execve failed */
}
