// shutdown(8) — sysvinit-style system shutdown for the Rux Ubuntu image.
//
// Sequence (the classic init signal path):
//   1. announce the shutdown on the console;
//   2. sync();
//   3. SIGTERM to PID 1 — udesk-init terminates every user process and
//      exits (the graceful half of the cascade);
//   4. wait (bounded) for init to exit;
//   5. reboot(2) with RB_AUTOBOOT / RB_POWER_OFF — the kernel cascade
//      re-checks init, syncs the block device and drives SBI reset /
//      shutdown, which exits QEMU.
//
// Usage: shutdown [-h|-P|-H|-r|-k] now|+m|hh:mm [message]
//   -h/-P  power off (default)      -H  halt
//   -r     reboot                    -k  "just warn", no real shutdown
// Time and message arguments are accepted for script compatibility; any
// non-immediate delay is clamped to "now" (no wall-clock scheduling).
#include <unistd.h>
#include <sys/reboot.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>

#define LINUX_REBOOT_CMD_RESTART  0x01234567
#define LINUX_REBOOT_CMD_HALT     0xcdef0123
#define LINUX_REBOOT_CMD_POWEROFF 0x4321fedc

int main(int argc, char **argv) {
    int cmd = LINUX_REBOOT_CMD_POWEROFF;
    int warn_only = 0;

    int i = 1;
    for (; i < argc && argv[i][0] == '-' && argv[i][1]; i++) {
        if (!strcmp(argv[i], "-h") || !strcmp(argv[i], "-P")) {
            cmd = LINUX_REBOOT_CMD_POWEROFF;
        } else if (!strcmp(argv[i], "-H")) {
            cmd = LINUX_REBOOT_CMD_HALT;
        } else if (!strcmp(argv[i], "-r")) {
            cmd = LINUX_REBOOT_CMD_RESTART;
        } else if (!strcmp(argv[i], "-k")) {
            warn_only = 1;
        } else {
            fprintf(stderr, "shutdown: unknown flag %s\n", argv[i]);
            return 2;
        }
    }
    // time argument: "now" (required for the immediate path), "+m" delay
    // or "hh:mm" — all clamped to "now" (no wall-clock scheduling).
    if (i < argc && strcmp(argv[i], "now")
        && argv[i][0] != '+' && (argv[i][0] < '0' || argv[i][0] > '9')) {
        fprintf(stderr, "shutdown: invalid time %s\n", argv[i]);
        return 2;
    }

    setvbuf(stdout, NULL, _IONBF, 0);
    printf("\nShutdown scheduled (now)\n");
    if (i + 1 < argc) { // optional free-text message
        printf("%s ", argv[i + 1]);
        for (int j = i + 2; j < argc; j++) printf("%s ", argv[j]);
        printf("\n");
    }
    if (warn_only) {
        printf("shutdown: -k given, NOT shutting down\n");
        return 0;
    }

    printf("Shutdown: sending SIGTERM to all processes\n");
    sync();

    // Init signal path: tell PID 1 to tear the session down, then wait
    // (bounded) for it to exit before pulling the kernel lever.
    if (kill(1, SIGTERM) == 0) {
        for (int w = 0; w < 50; w++) { // 5s in 100ms slices
            if (kill(1, 0) < 0 && errno == ESRCH) break;
            usleep(100 * 1000);
        }
        if (kill(1, 0) == 0)
            printf("Shutdown: init still up, kernel cascade will signal again\n");
        else
            printf("Shutdown: init exited cleanly\n");
    }

    switch (cmd) {
    case LINUX_REBOOT_CMD_RESTART:
        printf("Shutdown: rebooting system\n");
        break;
    case LINUX_REBOOT_CMD_HALT:
        printf("Shutdown: halting system\n");
        break;
    default:
        printf("Shutdown: powering off\n");
        break;
    }
    reboot(cmd); // never returns on success (kernel cascade + SBI)

    fprintf(stderr, "Shutdown: reboot(2) failed: errno=%d\n", errno);
    return 1;
}
