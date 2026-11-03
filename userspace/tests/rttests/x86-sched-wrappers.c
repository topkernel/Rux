/* x86-sched-wrappers.c — override the ENOSYS stubs that ship in the
 * x86_64-rux-linux-musl toolchain's libc.a (qa-suites lane finding:
 * sched_getparam/getscheduler/setparam/setscheduler and utimensat are
 * 25-byte stubs returning -ENOSYS; the kernel implements all of them —
 * raw SYS_sched_getparam succeeds). Linked as explicit objects ahead
 * of libc.a, these strong definitions win over the archive members.
 * Drop this file once the toolchain tarball is rebuilt without the
 * stubs.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <sched.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

int sched_getparam(pid_t pid, struct sched_param *param)
{
    return syscall(SYS_sched_getparam, pid, param);
}

int sched_getscheduler(pid_t pid)
{
    return syscall(SYS_sched_getscheduler, pid);
}

int sched_setparam(pid_t pid, const struct sched_param *param)
{
    return syscall(SYS_sched_setparam, pid, param);
}

int sched_setscheduler(pid_t pid, int policy, const struct sched_param *param)
{
    return syscall(SYS_sched_setscheduler, pid, policy, param);
}

int utimensat(int dirfd, const char *pathname,
              const struct timespec times[2], int flags)
{
#ifdef SYS_utimensat
    return syscall(SYS_utimensat, dirfd, pathname, times, flags);
#else
    errno = ENOSYS;
    return -1;
#endif
}
