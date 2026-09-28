// udesk-init — PID 1 for the Ubuntu GUI session image.
//
// Mounts /proc, performs the proven one-shot /bin/true exec warmup (stabilises
// the first real exec after boot), then keeps the udesk framebuffer session
// alive on the console, respawning it if it ever exits.
#include <unistd.h>
#include <sys/mount.h>
#include <sys/wait.h>
#include <stdio.h>

int main(void) {
    mount("proc", "/proc", "proc", 0, 0);
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("udesk-init: boot ok\n");

    {
        char *w[] = {"/bin/true", NULL};
        char *ev[] = {"HOME=/root", "PATH=/bin:/usr/bin:/sbin", NULL};
        pid_t wp = fork();
        if (wp == 0) { execve(w[0], w, ev); _exit(127); }
        int wst; waitpid(wp, &wst, 0);
        printf("udesk-init: warmup st=%d\n", wst);
    }

    for (;;) {
        pid_t pid = fork();
        if (pid == 0) {
            char *av[] = {"/usr/bin/udesk", NULL};
            char *ev[] = {"HOME=/root", "PATH=/bin:/usr/bin:/sbin", "TERM=dumb", NULL};
            execve(av[0], av, ev);
            _exit(127);
        }
        int st;
        waitpid(pid, &st, 0);
        printf("udesk-init: udesk st=%d, respawning\n", st);
        sleep(1);
    }
}
