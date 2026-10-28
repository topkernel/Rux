/* cpuinfo_x86probe.c — x86 /proc/cpuinfo block-shape contract.
 *
 * Linux (arch/x86/kernel/cpu/proc.c show_cpuinfo) emits the `processor`
 * row EXACTLY ONCE per CPU block, as the block's FIRST row. Rux used to
 * duplicate it after cache_alignment (the pre-D-4 shape the conversion
 * preserved); this probe convicts or clears that.
 *
 * Checks per block: `processor` starts the block, `cache_alignment` ends
 * it, and the number of `processor` rows equals the number of blocks.
 * Final line: "PROBE-RESULT: PASS|FAIL <detail>".
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <sys/reboot.h>

static int failures;

static void fail(const char *what, const char *detail)
{
    printf("PROBE-FAIL %s: %s\n", what, detail);
    failures++;
}

int main(void)
{
    static char buf[65536];
    int fd = open("/proc/cpuinfo", O_RDONLY);
    if (fd < 0) {
        printf("PROBE-RESULT: FAIL open /proc/cpuinfo errno=%d\n", errno);
        return 1;
    }
    int total = 0, n;
    while (total < (int)sizeof(buf) - 1 &&
           (n = read(fd, buf + total, sizeof(buf) - 1 - total)) > 0)
        total += n;
    close(fd);
    if (total <= 0) {
        printf("PROBE-RESULT: FAIL empty /proc/cpuinfo\n");
        return 1;
    }
    buf[total] = 0;
    printf("===CPUINFO-X86: start\n");

    /* Count CPU blocks by their terminal row, and processor rows. */
    int blocks = 0, procs = 0, trailing_dup = 0;
    const char *p = buf;
    for (;;) {
        p = strstr(p, "cache_alignment");
        if (!p)
            break;
        blocks++;
        /* The text from this row's end to the next block (or EOF): a
         * duplicated `processor` row there is the bug. */
        const char *next = strstr(p + 1, "\nprocessor");
        const char *blk_end = strstr(p + 1, "\n\n");
        if (next && (!blk_end || next < blk_end)) {
            /* \nprocessor before the blank separator → inside this block
             * after cache_alignment: the duplicate shape. */
            trailing_dup++;
        }
        p += 15;
    }
    p = buf;
    for (;;) {
        p = strstr(p, "processor");
        if (!p)
            break;
        /* Row start check: beginning of file or right after \n, then a
         * tab (the key padding) and the colon. */
        if ((p == buf || p[-1] == '\n') && p[9] == '\t')
            procs++;
        p += 9;
    }

    printf("===CPUINFO-X86: blocks=%d processor_rows=%d trailing_dup=%d\n",
           blocks, procs, trailing_dup);

    if (blocks == 0)
        fail("blocks", "no cache_alignment row found");
    if (procs != blocks) {
        char d[80];
        snprintf(d, sizeof(d), "processor rows (%d) != blocks (%d)", procs, blocks);
        fail("one-processor-per-block", d);
    }
    if (trailing_dup != 0) {
        char d[80];
        snprintf(d, sizeof(d), "%d block(s) carry processor AFTER cache_alignment", trailing_dup);
        fail("no-trailing-processor", d);
    }
    /* First row of the file must be the first block's processor row. */
    if (strncmp(buf, "processor\t: 0\n", 14) != 0)
        fail("processor-first", "first block does not start with 'processor\\t: 0'");

    if (failures == 0)
        printf("PROBE-RESULT: PASS\n");
    else
        printf("PROBE-RESULT: FAIL (%d)\n", failures);
    fflush(stdout);
    sleep(1);
    reboot(0x4321fedc); /* LINUX_REBOOT_CMD_POWER_OFF (the dfx-x86 idiom) */
    return 0;
}
