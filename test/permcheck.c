/*
 * permcheck.c — dump the kernel-side access-token permission bitmap.
 *
 * AccessTokenKit::VerifyAccessToken for native (same-process) tokens uses
 * the KERNEL fast path: ACCESS_TOKENID_GET_PERMISSION on
 * /dev/access_token_id, reading the per-token u32[64] bitmap that ATM
 * pushes via ACCESS_TOKENID_ADD_PERMISSIONS. If that bitmap is empty or
 * wrong, every native permission verify denies without ever reaching
 * ATM's own (authoritative) records — the b6 "DumpInfos verify
 * permission failed" shape.
 *
 * Usage: permcheck <hex-token> [max-opcode]
 *   Prints every granted opcode in [0, max-opcode) (default 2048) and a
 *   per-word hex dump of the bitmap as the kernel sees it.
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
#define ACCESS_TOKENID_GET_PERMISSION \
    _IOW(ACCESS_TOKEN_ID_IOCTL_BASE, 7, struct IoctlSetGetPermData)

struct IoctlSetGetPermData {
    uint32_t token;
    uint32_t opCode;
    uint8_t isGranted;
};

static const char *NODE = "/dev/access_token_id";

int main(int argc, char **argv)
{
    if (argc < 2) {
        fprintf(stderr, "usage: permcheck <hex-token> [max-opcode]\n");
        return 2;
    }
    uint32_t token = (uint32_t)strtoul(argv[1], NULL, 0);
    int maxop = (argc >= 3) ? atoi(argv[2]) : 2048;

    int fd = open(NODE, O_RDWR);
    if (fd < 0) {
        fprintf(stderr, "permcheck: open %s: %s\n", NODE, strerror(errno));
        return 3;
    }

    uint32_t words[64];
    memset(words, 0, sizeof(words));
    int granted_count = 0;
    for (int op = 0; op < maxop && op < 64 * 32; op++) {
        struct IoctlSetGetPermData d = { token, (uint32_t)op, 0 };
        if (ioctl(fd, ACCESS_TOKENID_GET_PERMISSION, &d) != 0) {
            fprintf(stderr, "permcheck: GET_PERMISSION op %d: %s\n", op,
                    strerror(errno));
            close(fd);
            return 4;
        }
        if (d.isGranted) {
            granted_count++;
            words[op / 32] |= 1u << (op % 32);
        }
    }
    close(fd);

    printf("permcheck: token 0x%x granted %d opcodes in [0,%d)\n", token,
           granted_count, maxop);
    printf("bitmap:");
    for (int w = 0; w < 8; w++) {
        printf(" w%d=%08x", w, words[w]);
    }
    printf("\n");
    if (granted_count == 0) {
        printf("permcheck: BITMAP EMPTY — ATM's ADD_PERMISSIONS never "
               "landed for this token\n");
    }
    return 0;
}
