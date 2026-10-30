/* paintshim — pid1 ELF shim for the painter ramdisk (the kernel's init
 * loader has no binfmt_script, so /rpaint cannot be a #! script).
 * 1. fork: child execs the scanpaint painter (inherits pid1's console
 *    stdio, so its log rides the serial).
 * 2. parent execs /init (argv[0]="/init" exactly as the kernel's own
 *    candidate exec would), handing control to OH init.
 */
#include <unistd.h>
int main(void)
{
    if (fork() == 0) {
        execl("/scanpaint", "scanpaint", "720", "1280", (char *)0);
        _exit(127);
    }
    execl("/init", "/init", (char *)0);
    return 127;
}
