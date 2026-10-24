#include <fcntl.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>

static int puts_(const char *s) { return write(1, s, strlen(s)); }
static void putu_(unsigned long v) {
    char b[24]; int i = 24;
    if (!v) { puts_("0"); return; }
    while (v && i) { b[--i] = '0' + (v % 10); v /= 10; }
    write(1, b + i, 24 - i);
}
static long long now_us(void) {
    struct timespec ts;
    syscall(SYS_clock_gettime, CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000000LL + ts.tv_nsec / 1000;
}

#define NFILES 6
static volatile unsigned long sink;

static int fault_file(const char *path, unsigned long *out_avg, unsigned long *out_max) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return -1;
    struct stat st;
    if (fstat(fd, &st) < 0 || st.st_size < 65536) { close(fd); return -1; }
    char *p = mmap(0, (size_t)st.st_size, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED) { close(fd); return -1; }
    size_t npages = (size_t)st.st_size / 4096;
    unsigned long long acc = 0; unsigned long mx = 0;
    for (size_t i = 0; i < npages; i++) {
        long long t0 = now_us();
        sink += p[i * 4096];
        unsigned long dt = (unsigned long)(now_us() - t0);
        acc += dt; if (dt > mx) mx = dt;
    }
    *out_avg = (unsigned long)(acc / npages);
    *out_max = mx;
    munmap(p, (size_t)st.st_size);
    close(fd);
    return (int)npages;
}

int main(void) {
    static const char *files[NFILES] = {
        "/test/d0.bin", "/test/d1.bin", "/test/d2.bin",
        "/test/d3.bin", "/test/d4.bin", "/test/d5.bin"};
    long long t0 = now_us();
    for (int i = 0; i < NFILES; i++) {
        pid_t pid = fork();
        if (pid == 0) {
            unsigned long avg, mx;
            int np = fault_file(files[i], &avg, &mx);
            puts_("FAULTLAT2 child "); putu_(i); puts_(" pages=");
            putu_(np > 0 ? (unsigned long)np : 0); puts_(" avg=");
            putu_(avg); puts_("us max="); putu_(mx); puts_("us\n");
            _exit(0);
        }
    }
    for (int i = 0; i < NFILES; i++) wait(0);
    unsigned long wall = (unsigned long)((now_us() - t0) / 1000);
    puts_("FAULTLAT2 all children done in "); putu_(wall); puts_("ms\n");
    puts_("FAULTLAT2 RESULT: DONE\n");
    return 0;
}
