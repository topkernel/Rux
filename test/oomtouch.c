// oomtouch.c — deterministic OOM victim for the trace-event smoke.
//
// Touches anonymous memory in 1 MiB chunks until the kernel's OOM
// killer kills it. The kill path fires the oom:mark_victim tracepoint
// (the victim IS this process — the biggest anon consumer in the
// guest) and the signal:signal_generate tracepoint (the SIGKILL,
// thread-group-directed). The stress lane's fake-OOM hunts get the
// same marker pair on every victim.
//
// Build: the trace-smoke3 recipe links it musl-static (the rootfs
// libc — see trace-smoke2.sh for why the distro glibc cannot be
// used: its memset carries RVV the standard QEMU cpu lacks).

#include <stdlib.h>
#include <string.h>

int main(void)
{
    size_t chunk = 1u << 20;
    for (size_t i = 0; i < 8192; i++) { /* up to 8 GiB virtual */
        char *p = malloc(chunk);
        if (!p)
            continue;
        memset(p, 1, chunk);
    }
    /* Not reached in a 1 GiB guest: the OOM kill lands first. */
    return 0;
}
