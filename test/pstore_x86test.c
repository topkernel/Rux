/* pstore_x86test.c — x86_64 pstore anchor + reset-path survival probe
 * (as init).
 *
 * One binary, three boots (a guest reset does not clear RAM, so the
 * reserved region keeps every capture the slots hold):
 *
 *   boot 1: no records -> pin the reset method to the 8042 PULSE only
 *           (debugfs /sys/kernel/debug/dfx/reset-method), print the
 *           boot-1 marker, then crash via sysrq 'c' (the cmdline
 *           carries panic=3). The record survives the pulse reset.
 *
 *   boot 2: boot-1's record exists but is NOT read (an unread record
 *           must survive); pin the reset method to the TRIPLE FAULT,
 *           print the boot-2 marker, crash again. The new record
 *           survives the triple-fault reset — the harsher CPU state
 *           wash the mission's survival clause names.
 *
 *   boot 3: /sys/fs/pstore holds TWO records: console-ramoops-0
 *           (newest = boot 2; its printk tail carries this probe's
 *           "triplefault" pin line) and console-ramoops-1 (boot 1; its
 *           tail carries the "8042" pin line). Both are read fully and
 *           checked; reads are REPEATABLE (a re-read of the newest is
 *           byte-identical and still openable); erasure follows the
 *           reference unlink contract — unlink(2) erases the record
 *           (pstore_unlink -> psi->erase): a re-open after the unlink
 *           answers ENOENT, a second unlink fails, and unlinking an
 *           ordinary sysfs attribute is refused (EPERM — sysfs files
 *           are kernel-owned; only the pstore family is removable).
 *
 * Built static (the pbench toolchain) and staged as /test/px by
 * test/pstore-x86-smoke.sh.
 */
#include <dirent.h>
#include <fcntl.h>
#include <string.h>
#include <unistd.h>

static void say(const char *s) { write(1, s, strlen(s)); }

static int write_file(const char *path, const char *data)
{
    int fd = open(path, O_WRONLY);
    if (fd < 0)
        return -1;
    ssize_t n = write(fd, data, strlen(data));
    close(fd);
    return (n > 0) ? 0 : -1;
}

/* Read a whole pstore record (payload <= 32 KiB) in one open. Reading
 * consumes nothing (the reference contract: erase is unlink-only), so
 * the file stays openable after this. Returns bytes read, or -1 when
 * the file is gone. */
static int read_record(const char *path, char *buf, int cap)
{
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return -1;
    ssize_t n = read(fd, buf, cap - 1);
    close(fd);
    if (n >= 0)
        buf[n] = '\0';
    return (int)n;
}

/* Bounded search (memmem semantics): the record is BINARY — the
 * printk syslog headers render kernel-thread comms as NUL bytes
 * ("pid(\0)"), so plain strstr would stop at the first one. */
static int count_occurrences(const char *haystack, int hlen, const char *needle)
{
    int count = 0;
    int nlen = (int)strlen(needle);
    if (nlen == 0 || hlen < nlen)
        return 0;
    for (int i = 0; i + nlen <= hlen; i++) {
        if (memcmp(haystack + i, needle, (size_t)nlen) == 0) {
            count++;
            i += nlen - 1;
        }
    }
    return count;
}

static char rec0[40960];
static char rec1[40960];

int main(void)
{
    static const char knob[] = "/sys/kernel/debug/dfx/reset-method";

    /* Record presence decides the boot's role. Boot 3 sees two files;
     * boots 1 and 2 see none (boot 2 must NOT read boot 1's record). */
    DIR *d = opendir("/sys/fs/pstore");
    int records = 0;
    if (d) {
        struct dirent *e;
        while ((e = readdir(d)) != NULL) {
            if (strncmp(e->d_name, "console-ramoops-", 16) == 0)
                records++;
        }
        closedir(d);
    }

    if (records >= 2) {
        /* ---- boot 3: verify both records, newest first, then the
         * unlink-erase contract. ---- */
        say("===PX3: two records resurfaced===\n");

        int n0 = read_record("/sys/fs/pstore/console-ramoops-0", rec0, sizeof(rec0));
        say(n0 >= 0 ? "===PX3: read console-ramoops-0===\n"
                    : "===PX3: FAIL open console-ramoops-0===\n");
        if (n0 < 0)
            goto done;

        /* Repeatable reads: a second full read must still succeed and
         * be byte-identical (the reference reads the same record
         * buffer until the file is unlinked). */
        static char again[40960];
        int n0b = read_record("/sys/fs/pstore/console-ramoops-0", again, sizeof(again));
        if (n0b == n0 && n0b >= 0 && memcmp(rec0, again, (size_t)n0) == 0)
            say("===PX3: re-read identical, still openable===\n");
        else
            say("===PX3: FAIL re-read differs or refused===\n");

        int n1 = read_record("/sys/fs/pstore/console-ramoops-1", rec1, sizeof(rec1));
        say(n1 >= 0 ? "===PX3: read console-ramoops-1===\n"
                    : "===PX3: FAIL open console-ramoops-1===\n");
        if (n1 < 0)
            goto done;

        /* Section forensics: the record = identity head + staged report
         * + console tail. Dump the first 400 and last 400 bytes so the
         * harness can see which sections made it. */
        say("===PX3: rec0 head dump===\n");
        write(1, rec0, n0 > 400 ? 400 : n0);
        say("\n===PX3: rec0 tail dump===\n");
        write(1, rec0 + (n0 > 400 ? n0 - 400 : 0), n0 > 400 ? 400 : 0);
        say("\n===PX3: dump end===\n");

        say(count_occurrences(rec0, n0, "Kernel panic - not syncing") > 0
                ? "===PX3: newest has panic banner===\n"
                : "===PX3: FAIL newest banner===\n");
        say(count_occurrences(rec0, n0, "Call Trace") > 0
                ? "===PX3: newest has call trace===\n"
                : "===PX3: FAIL newest trace===\n");
        /* Newest = boot 2: its console tail carries the triple-fault
         * pin line; boot 1's record carries the 8042 one. */
        say(count_occurrences(rec0, n0, "reset method pinned to triplefault") > 0
                ? "===PX3: newest is boot2 (triplefault path)===\n"
                : "===PX3: FAIL newest not boot2===\n");
        say(count_occurrences(rec1, n1, "reset method pinned to 8042") > 0
                ? "===PX3: oldest is boot1 (8042 path)===\n"
                : "===PX3: FAIL oldest not boot1===\n");

        /* Unlink-erase: unlink(2) removes the record (the reference's
         * only erase path). Re-open must then answer ENOENT, a second
         * unlink must fail, and an ordinary sysfs attribute must
         * refuse the unlink (EPERM). */
        int rc = unlink("/sys/fs/pstore/console-ramoops-0");
        say(rc == 0 ? "===PX3: unlinked newest===\n"
                    : "===PX3: FAIL unlink newest===\n");
        int fd = open("/sys/fs/pstore/console-ramoops-0", O_RDONLY);
        say(fd < 0 ? "===PX3: unlinked newest gone (ENOENT)===\n"
                   : "===PX3: FAIL unlinked newest still openable===\n");
        if (fd >= 0)
            close(fd);
        rc = unlink("/sys/fs/pstore/console-ramoops-0");
        say(rc < 0 ? "===PX3: second unlink fails===\n"
                   : "===PX3: FAIL second unlink succeeded===\n");
        rc = unlink("/sys/kernel/uevent_seqnum");
        say(rc < 0 ? "===PX3: ordinary sysfs attr unlink refused===\n"
                   : "===PX3: FAIL ordinary sysfs attr unlinked===\n");
        rc = unlink("/sys/fs/pstore/console-ramoops-1");
        say(rc == 0 ? "===PX3: unlinked oldest===\n"
                    : "===PX3: FAIL unlink oldest===\n");
        d = opendir("/sys/fs/pstore");
        if (d) {
            struct dirent *e;
            int left = 0;
            while ((e = readdir(d)) != NULL) {
                if (e->d_name[0] != '.')
                    left++;
            }
            closedir(d);
            say(left == 0 ? "===PX3: directory empty (both unlinked)===\n"
                          : "===PX3: FAIL directory not empty===\n");
        }
        say("===PX3: record end===\n");
    done:
        for (;;)
            pause();
    }

    /* ---- boots 1 and 2: pin one reset path, then crash. ---- */
    if (write_file(knob, records == 0 ? "8042\n" : "triplefault\n") != 0) {
        say("===PX: FAIL writing reset-method knob===\n");
        return 1;
    }
    if (records == 0)
        say("===PX1: crashing via sysrq c, reset pinned to 8042===\n");
    else
        say("===PX2: crashing via sysrq c, reset pinned to triplefault===\n");
    if (write_file("/proc/sysrq-trigger", "c") != 0) {
        say("===PX: FAIL sysrq write===\n");
        return 1;
    }
    for (;;)
        pause();
    return 0;
}
