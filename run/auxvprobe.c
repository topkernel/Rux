/* auxvprobe — dump the ELF auxiliary vector our kernel provides.
 *
 * Compare against the reference kernel's: the AT_HWCAP/AT_PLATFORM/
 * AT_SECURE/AT_RANDOM entries drive glibc/ld.so CPU and security
 * decisions that LLVM inherits.
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <sys/auxv.h>

int main(void)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    static const struct { long key; const char *name; } keys[] = {
        { AT_PHDR, "AT_PHDR" }, { AT_PHENT, "AT_PHENT" },
        { AT_PHNUM, "AT_PHNUM" }, { AT_PAGESZ, "AT_PAGESZ" },
        { AT_BASE, "AT_BASE" }, { AT_FLAGS, "AT_FLAGS" },
        { AT_ENTRY, "AT_ENTRY" }, { AT_UID, "AT_UID" },
        { AT_EUID, "AT_EUID" }, { AT_GID, "AT_GID" },
        { AT_EGID, "AT_EGID" }, { AT_SECURE, "AT_SECURE" },
        { AT_RANDOM, "AT_RANDOM" }, { AT_HWCAP, "AT_HWCAP" },
        { AT_CLKTCK, "AT_CLKTCK" }, { AT_PLATFORM, "AT_PLATFORM" },
        { AT_EXECFN, "AT_EXECFN" },
        { AT_SYSINFO_EHDR, "AT_SYSINFO_EHDR" },
    };
    for (unsigned i = 0; i < sizeof(keys)/sizeof(keys[0]); i++) {
        unsigned long v = getauxval(keys[i].key);
        printf("AUXV %-16s = %#lx (%lu)\n", keys[i].name, v, v);
    }
    printf("AUXV-DONE\n");
    return 0;
}
