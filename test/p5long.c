/* p5long.c — init wrapper for the wwcensus discriminator boot (leg 46):
 * execve the probe in its "longp5" mode (duration-bounded p5c2/p5c3
 * windows with serial markers). Boot the kernel with dfx=wwcensus on the
 * cmdline (pbench_boot.py --long-p5 --append-extra "dfx=wwcensus") so
 * the 1 Hz WW heartbeat lines (wakes/picks/avg_run/stranded) and the
 * wake-refusal prints between the markers are the census measurement.
 * Built static by pbench_boot.py --long-p5 and staged as /test/p5long;
 * init becomes /test/p5long.
 */
#include <stdio.h>
#include <string.h>
#include <unistd.h>

/* The MODE is the wrapper's own staged basename: /test/p5long execs
 * "longp5" (cold windows), /test/p5longwarm execs "longp5warm" (board
 * p5 section first, then windows + placement variants) — one source,
 * two staged names. */
int main(int argc, char **argv)
{
    static char mode[32] = "longp5";
    if (argc > 0 && argv[0] && strrchr(argv[0], '/')) {
        const char *base = strrchr(argv[0], '/') + 1;
        /* /test/p5long -> "longp5"; /test/p5longwarm -> "longp5warm" */
        if (strncmp(base, "p5long", 6) == 0)
            snprintf(mode, sizeof(mode), "longp5%s", base + 6);
    }
    char *cargv[] = { "/test/pbench", mode, 0 };
    char *envp[] = { 0 };
    execve(cargv[0], cargv, envp);
    return 127; /* execve failed */
}
