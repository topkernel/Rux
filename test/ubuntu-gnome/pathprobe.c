/* pathprobe.c — static init wrapper for the /dev/input path tests.
 *
 * The kernel init path does not exec #! scripts (init=/rux-*.sh boots
 * die with "ELF load fail", r10pathprobe) — so this static musl ELF
 * wraps `sh -c <tests>` as PID1. The tests themselves run with the
 * image's own dash + coreutils (glibc), reproducing exactly the
 * realpath()/stat() syscall pattern libinput uses when
 * libinput_path_add_device rejects /dev/input/eventN with
 * "client bug: Invalid path".
 */
#include <unistd.h>
#include <stdio.h>

int main(void)
{
    const char *cmd =
        "echo PROBE-begin; "
        "echo ---ls-dev; ls -la /dev | head -12; "
        "echo ---ls-input; ls -la /dev/input; "
        "echo ---stat0; stat /dev/input/event0; "
        "echo ---realpath; realpath /dev/input/event0; echo realpath_rc=$?; "
        "echo ---readlink; readlink -f /dev/input/event0; echo readlink_rc=$?; "
        "echo ---lstat; ls -la /dev/input/event0; "
        "echo ---open-read; timeout 2 cat /dev/input/event0 >/dev/null; "
        "echo cat_rc=$?; "
        "echo PROBE-done";
    printf("[pathprobe] launching sh -c tests\n");
    fflush(stdout);
    execl("/bin/sh", "sh", "-c", cmd, (char *)0);
    perror("[pathprobe] exec /bin/sh");
    return 1;
}
