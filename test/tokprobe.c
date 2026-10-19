/*
 * tokprobe.c — bisect the tokwrap SIGSEGV: step-printed GET/SET
 * TOKENID sequence against /dev/access_token_id.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#define BASE 'A'
#define GET_TOKENID _IOR(BASE, 1, uint64_t)
#define SET_TOKENID _IOW(BASE, 2, uint64_t)
#define GET_FTOKENID _IOR(BASE, 3, uint64_t)
#define SET_FTOKENID _IOW(BASE, 4, uint64_t)

int main(int argc, char **argv)
{
    fprintf(stderr, "step0: open\n");
    int fd = open("/dev/access_token_id", O_RDWR);
    if (fd < 0) {
        fprintf(stderr, "open: %s\n", strerror(errno));
        return 1;
    }
    uint64_t old = 0;
    fprintf(stderr, "step1: GET_TOKENID req=%#lx\n", (unsigned long)GET_TOKENID);
    int r = ioctl(fd, GET_TOKENID, &old);
    fprintf(stderr, "step2: GET ret=%d old=%#llx (%s)\n", r,
            (unsigned long long)old, r ? strerror(errno) : "ok");

    uint64_t tok = (argc > 1) ? strtoull(argv[1], NULL, 0)
                              : (0x28000000ull | (getpid() & 0xFFFFFull));
    fprintf(stderr, "step3: SET_TOKENID req=%#lx val=%#llx\n",
            (unsigned long)SET_TOKENID, (unsigned long long)tok);
    r = ioctl(fd, SET_TOKENID, &tok);
    fprintf(stderr, "step4: SET ret=%d (%s)\n", r, r ? strerror(errno) : "ok");

    uint64_t back = 0;
    r = ioctl(fd, GET_TOKENID, &back);
    fprintf(stderr, "step5: readback ret=%d val=%#llx\n", r,
            (unsigned long long)back);

    uint64_t ftok = 0;
    r = ioctl(fd, GET_FTOKENID, &ftok);
    fprintf(stderr, "step6: GET_FTOKENID ret=%d val=%#llx\n", r,
            (unsigned long long)ftok);

    close(fd);
    fprintf(stderr, "step7: done\n");
    return 0;
}
