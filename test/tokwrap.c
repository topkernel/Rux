/*
 * tokwrap.c — start an OH tool with a chosen access token.
 *
 * The screen-unlock gate in AbilityManagerService::StartAbility
 * (UnlockScreenManager::UnlockScreen) only engages for callers whose
 * IPC token reads TOKEN_SHELL while developer mode is on:
 *
 *     if (!isShellCall || !isDeveloperMode) return true;
 *
 * hdc-shell processes inherit hdcd's shell token (0x300a0acb on this
 * image), so every `aa start` from hdc dies with
 * ERR_UNLOCK_SCREEN_FAILED_IN_DEVELOPER_MODE. The kernel keeps the
 * per-pid token in the access_token registry (keyed by tgid, survives
 * exec), and /dev/access_token_id ACCESS_TOKENID_SET_TOKENID is a
 * privileged (uid 0) self-service ioctl — hdcd's children run as root.
 *
 * tokwrap re-stamps its own token to a native-range value (type bits
 * 27-28 = 0b01, matching the image's nativetoken.json entries) and
 * execs the requested command. The binder driver then reports a
 * TOKEN_NATIVE sender and the unlock gate passes for the call.
 *
 * Usage: tokwrap [-t <hex-token>] <cmd> [args...]
 *   default token: 0x28000000 | getpid()
 */
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#define ACCESS_TOKEN_ID_IOCTL_BASE 'A'
#define TOKEN_U64(n) _IOW(ACCESS_TOKEN_ID_IOCTL_BASE, n, uint64_t)
#define ACCESS_TOKENID_SET_TOKENID TOKEN_U64(2)
#define ACCESS_TOKENID_GET_TOKENID _IOR(ACCESS_TOKEN_ID_IOCTL_BASE, 1, uint64_t)

static const char *NODE = "/dev/access_token_id";

int main(int argc, char **argv)
{
    uint64_t token = 0;
    int argi = 1;

    if (argc >= 3 && strcmp(argv[1], "-t") == 0) {
        token = strtoull(argv[2], NULL, 0);
        argi = 3;
    }
    if (argc - argi < 1) {
        fprintf(stderr, "usage: tokwrap [-t hex-token] cmd [args...]\n");
        return 2;
    }

    int fd = open(NODE, O_RDWR);
    if (fd < 0) {
        fprintf(stderr, "tokwrap: open %s: %s\n", NODE, strerror(errno));
        return 3;
    }

    uint64_t old = 0;
    if (ioctl(fd, ACCESS_TOKENID_GET_TOKENID, &old) != 0) {
        old = 0;
    }
    if (token == 0) {
        token = 0x28000000ull | ((uint64_t)getpid() & 0xFFFFFull);
    }
    if (ioctl(fd, ACCESS_TOKENID_SET_TOKENID, &token) != 0) {
        fprintf(stderr, "tokwrap: SET_TOKENID(%#llx): %s (uid=%d)\n",
                (unsigned long long)token, strerror(errno), getuid());
        close(fd);
        return 4;
    }
    close(fd);

    fprintf(stderr, "tokwrap: pid %d uid %d token %#llx -> %#llx, exec %s\n",
            getpid(), getuid(), (unsigned long long)old,
            (unsigned long long)token, argv[argi]);
    execvp(argv[argi], &argv[argi]);
    fprintf(stderr, "tokwrap: exec %s: %s\n", argv[argi], strerror(errno));
    return 5;
}
