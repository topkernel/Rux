/* modchurn_lib.c — the churned module for the modchurn fleet member.
 *
 * OH leg-22 bssslack2 shape (SC-SESSIONS.txt): a dlopen'd module's
 * anon bss VMA carried FOREIGN data in pages no code ever wrote —
 * string fragments + a live userspace pointer beyond ph4 memsz. This
 * library reproduces the exact layout: a constructor that initializes
 * only the FIRST pages of a large .bss, leaving the rest (plus the
 * page-rounding slack past the array end) addressed by nothing. Every
 * byte the driver reads there must be zero; anything else is a frame
 * the recycler handed out dirty (or double-mapped).
 *
 * Build: run/fleet/bin/riscv64-musl-gcc -shared -fPIC -O1 \
 *            -o libmodchurn.so modchurn_lib.c
 */
#include <stdint.h>

/* 256 KiB of bss. The constructor writes page 0 (marker + fill bytes);
 * pages 1..63 and the tail-slack past the array end stay untouched. */
__attribute__((aligned(4096))) char churn_bss[0x40000];

/* A small data section analogue (writable, initialized) so the module
 * has a PT_LOAD besides bss — closer to a real .so shape. */
volatile uint64_t churn_data_anchor = 0xC0FFEE1234567890UL;

/* Another anon consumer the ctor deliberately half-writes: mimics the
 * hicollie watchdog object that had sane ctor fields next to stale
 * never-written globals. */
__attribute__((aligned(4096))) char churn_bss2[0x10000];

__attribute__((constructor))
static void churn_ctor(void)
{
    for (int i = 0; i < 4096; i++)
        churn_bss[i] = (char)(0xA0 | (i & 0x0F));
    churn_bss2[0] = 0x5A;
    churn_data_anchor = 0xD1CEB0CA00000042UL;
}
