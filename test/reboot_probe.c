// reboot_probe — drive the kernel reboot(2) (NR 142) cascade directly.
//
// Prints markers around a raw reboot(LINUX_REBOOT_CMD_RESTART) call:
// on success the kernel takes over (SIGTERM init -> wait -> sync ->
// SBI reset) and the process never returns from the syscall — QEMU
// itself exits via the SBI system reset.
//
// Build (static, raw syscall — no libc reboot wrapper needed):
//   riscv64-linux-gnu-gcc -static -O2 -o reboot_probe reboot_probe.c
#include <stdio.h>
#include <errno.h>
#include <unistd.h>
#include <sys/syscall.h>

#define __NR_rux_reboot 142
#define LINUX_REBOOT_MAGIC1 0xfee1dead
#define LINUX_REBOOT_MAGIC2 672274793L
#define LINUX_REBOOT_CMD_RESTART 0x01234567

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("reboot_probe: calling reboot(RB_AUTOBOOT)\n");
    long r = syscall(__NR_rux_reboot, LINUX_REBOOT_MAGIC1,
                     LINUX_REBOOT_MAGIC2, LINUX_REBOOT_CMD_RESTART);
    printf("reboot_probe: FAILED rc=%ld errno=%d (cascade returned)\n", r, errno);
    return 1;
}
