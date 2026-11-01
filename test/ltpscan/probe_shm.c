/* MAP_SHARED file coherence across exec (LTP IPC pattern):
 * parent mmaps a file MAP_SHARED, fork+execs a copy of THIS binary which
 * re-opens and re-mmaps the same file, writes a magic; parent must observe
 * the write through its ORIGINAL mapping.
 * usage: probe_shm <file> [--child]
 */
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#define MAGIC_P 0x11223344u
#define MAGIC_C 0x55667788u

int main(int argc, char **argv)
{
    if (argc >= 3 && strcmp(argv[2], "--child") == 0) {
        int fd = open(argv[1], O_RDWR);
        if (fd < 0) { printf("child: open ERR\n"); return 1; }
        volatile unsigned *m = mmap(0, 4096, PROT_READ | PROT_WRITE,
                                    MAP_SHARED, fd, 0);
        if (m == MAP_FAILED) { printf("child: mmap ERR\n"); return 1; }
        if (m[0] != MAGIC_P) {
            printf("child: parent magic NOT visible: %08x (want %08x)\n",
                   m[0], MAGIC_P);
            /* fall through: still write ours */
        } else {
            printf("child: saw parent magic OK\n");
        }
        m[1] = MAGIC_C;
        printf("child: wrote magic\n");
        return 0;
    }

    const char *path = "/tmp/probe_shm.bin";
    if (argc >= 2) path = argv[1];
    unlink(path);
    int fd = open(path, O_CREAT | O_RDWR, 0644);
    if (fd < 0) { printf("parent: open ERR\n"); return 1; }
    ftruncate(fd, 4096);
    volatile unsigned *m = mmap(0, 4096, PROT_READ | PROT_WRITE,
                                MAP_SHARED, fd, 0);
    if (m == MAP_FAILED) { printf("parent: mmap ERR\n"); return 1; }
    m[0] = MAGIC_P;
    m[1] = 0;

    pid_t pid = fork();
    if (pid == 0) {
        char self[256];
        readlink("/proc/self/exe", self, sizeof(self) - 1);
        char *args[] = { self, (char *)path, "--child", NULL };
        execv(self, args);
        printf("child: exec ERR\n");
        _exit(1);
    }
    /* poll for child write through the original mapping */
    for (int i = 0; i < 100; i++) {
        if (m[1] == MAGIC_C) {
            printf("parent: saw child magic via mmap: OK\n");
            break;
        }
        usleep(100000); /* 100ms x 100 = 10s max */
    }
    if (m[1] != MAGIC_C) {
        /* fallback: is it visible after fresh re-map? */
        void *fresh = mmap(0, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        printf("parent: child magic via ORIGINAL mmap: %08x; via fresh mmap: %08x\n",
               m[1], ((volatile unsigned *)fresh)[1]);
    }
    int st; waitpid(pid, &st, 0);
    printf("parent: done\n");
    return 0;
}
