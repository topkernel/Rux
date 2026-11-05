/* unbuf.c — generic per-call-flushed stdout shim for the kselftest
 * timers binaries (build with -DUNBUF_TARGET='"<test>.c"').
 *
 * Why: on the serial (non-tty) console stdout is fully buffered, so a
 * stalled test (posix_timers, threadtest wedges) leaves its progress
 * invisible — the round-3 logs showed ZERO test output for both. This
 * shim flushes after every printf/vprintf so the last printed line
 * names the stall leg. kselftest.h prints only via printf/vprintf, so
 * wrapping those two covers the whole TAP stream. The upstream test
 * sources are compiled unmodified.
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

static int uvprintf(const char *fmt, va_list ap)
{
    int r = vfprintf(stdout, fmt, ap);

    fflush(stdout);
    return r;
}

#define printf uprintf
#define vprintf uvprintf

#include UNBUF_TARGET
