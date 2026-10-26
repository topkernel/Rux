/* inputcat.c — static evdev event reader for the virtio-input delivery
 * hunt (leg-7).
 *
 * State of the evidence: Xorg (r11ev, xf86-evdev) holds both
 * /dev/input/event{0,1} open and polls them continuously — the kernel's
 * poll path drains the virtqueues around the clock — yet QMP send-key /
 * input-send-event probes produce ZERO INPTRC lines and the on-screen
 * cursor never moves. Config-space enumeration works (Xorg sees the
 * vendor/product/abs caps), so this isolates the question to the EVENTQ
 * path: avail-ring submission -> QEMU writes the event -> used-ring ->
 * read_event(). That path has never positively delivered an event on
 * this kernel.
 *
 * This wrapper runs as init (the kernel init path cannot exec #!
 * scripts) and spawns a reader that opens BOTH nodes and prints every
 * input_event it reads ({sec,usec,type,code,value}) to the console.
 * fire.py --keyprobe/--mousemove provide the deterministic stimulus;
 * anything printed here proves the kernel delivery path end-to-end,
 * silence indicts it (kernel/QEMU side, not Xorg).
 */
#include <stdio.h>
#include <unistd.h>
#include <fcntl.h>
#include <poll.h>
#include <string.h>
#include <sys/wait.h>

struct input_event {
    long sec, usec;
    unsigned short type, code;
    int value;
};

int main(void)
{
    printf("[inputcat] spawning reader\n");
    fflush(stdout);
    pid_t r = fork();
    if (r == 0) {
        int f0 = open("/dev/input/event0", O_RDONLY | O_NONBLOCK);
        int f1 = open("/dev/input/event1", O_RDONLY | O_NONBLOCK);
        printf("[reader] open rc %d %d\n", f0, f1);
        if (f0 < 0 || f1 < 0)
            _exit(1);
        struct pollfd pf[2] = {
            { .fd = f0, .events = POLLIN },
            { .fd = f1, .events = POLLIN },
        };
        long n0 = 0, n1 = 0;
        for (;;) {
            int rc = poll(pf, 2, 5000);
            if (rc < 0) {
                printf("[reader] poll rc=%d\n", rc);
                continue;
            }
            if (rc == 0) {
                printf("[reader] idle (events kbd=%ld ptr=%ld)\n", n0, n1);
                fflush(stdout);
                continue;
            }
            struct input_event ev;
            if (pf[0].revents & POLLIN) {
                while (read(f0, &ev, sizeof ev) == (int)sizeof ev) {
                    n0++;
                    printf("[reader] KBD t=%ld.%06ld type=%u code=%u val=%d\n",
                           ev.sec, ev.usec, ev.type, ev.code, ev.value);
                }
            }
            if (pf[1].revents & POLLIN) {
                while (read(f1, &ev, sizeof ev) == (int)sizeof ev) {
                    n1++;
                    printf("[reader] PTR t=%ld.%06ld type=%u code=%u val=%d\n",
                           ev.sec, ev.usec, ev.type, ev.code, ev.value);
                }
            }
            fflush(stdout);
        }
    }
    int st;
    waitpid(r, &st, 0);
    printf("[inputcat] reader exited st=%d\n", st);
    return 0;
}
