/* pbench_p3d_arm.c — wrapper that turns the P3d concurrent-fault row on
 * (ledger 8.10.27 instrument debt: env P3D=1) and execve's the probe
 * unchanged. Built static by pbench_boot.py --p3d and staged as
 * /test/pbench_p3d; the boot's init becomes /test/pbench_p3d, so the
 * stock side is the same kernel, same image recipe without the row.
 * The env must come from THIS wrapper: the kernel's init envp is a
 * fixed list (init.rs) and never passes arbitrary cmdline tokens.
 */
#include <unistd.h>

int main(void)
{
    char *argv[] = { "/test/pbench", 0 };
    char *envp[] = { "P3D=1", 0 };
    write(1, "[arm] P3D=1\n", 12);
    execve(argv[0], argv, envp);
    return 127; /* execve failed */
}
