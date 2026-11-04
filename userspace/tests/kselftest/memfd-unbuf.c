/* memfd-unbuf.c — build shim: memfd_test.c with per-call flushed stdout.
 *
 * Why: on the serial (non-tty) console the stock binary's stdout is
 * fully buffered; when a child aborts (rc=134) the last flushed print
 * can be several tests behind the real abort site (round-3 F-4 was
 * initially mis-attributed to SEAL-FUTURE-WRITE for exactly this
 * reason — the kernel-side log showed the fault in the nested-sysctl
 * child). This shim wraps printf so every line hits the console the
 * moment it is printed; the last line before the crash names the leg.
 * The upstream source itself is compiled unmodified.
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdarg.h>

static int uprintf(const char *fmt, ...)
{
    va_list ap;
    int r;

    va_start(ap, fmt);
    r = vfprintf(stdout, fmt, ap);
    va_end(ap);
    fflush(stdout);
    return r;
}

#define printf uprintf

#include "memfd_test.c"
