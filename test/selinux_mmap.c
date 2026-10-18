// selinux_mmap.c — regression probe for the devfs NULL-deref panic
// (badaddr 0xa7) triggered by a NON-devfs file-backed mmap fault.
//
// Bug shape (OH cfi5/cfi6 boots, ~7 min in): composer_host /
// render_service's libselinux selinux_status_open() mmaps
// /sys/fs/selinux/status MAP_SHARED; the first fault's effective-size
// computation in handle_mm_fault called
// fs::devfs::devfs_reg_current_size() for EVERY file-backed VMA — and
// selinuxfs stores a NODES table INDEX (status = 11) in
// inode.private_data. The devfs helper cast it to *const DevfsEntry and
// read entry_type at 11 + 0x9c = 0xa7 → KERNPANIC.
//
// The probe must, on a FIXED kernel:
//   P1  mount selinuxfs, mmap `status` MAP_SHARED, read all 20 bytes
//       (version byte must be 1) — on the broken kernel this is the
//       instant KERNPANIC.
//   P2  same via MAP_PRIVATE (private_data is the same small integer;
//       the generic fault path must serve the page).
//   P3  mmap a procfs file MAP_PRIVATE and read it (procfs stores a pid
//       / node pointer in private_data — same foreign-inode class).
//   P4  devfs shared-backing regression (the machinery the buggy helper
//       serves): create /dev/__parameters__/probe_ws, ftruncate 64K,
//       mmap MAP_SHARED, memcpy a pattern, fault past a LATER
//       ftruncate growth (shared_grow path), and have a forked child
//       read the same mapping coherently.
//
// Verdict line: "SELINUX-MMAP: PASS" / individual FAIL lines.

#define _GNU_SOURCE
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#include <errno.h>

static int fails = 0;

#define CHECK(cond, name, fmt, ...) do {                                  \
    if (cond) {                                                           \
        printf("[ok] %s\n", name);                                        \
    } else {                                                              \
        fails++;                                                          \
        printf("FAIL %s " fmt "\n", name, ##__VA_ARGS__);                 \
    }                                                                     \
} while (0)

static int p1_selinuxfs_shared(void)
{
    // Mount selinuxfs (kernel keys the mount on the fstype string).
    if (mount("selinuxfs", "/selinux", "selinuxfs", 0, NULL) != 0) {
        // Already mounted is fine; anything else is a fail.
        if (errno != EBUSY && errno != EEXIST) {
            printf("FAIL p1 mount selinuxfs errno=%d\n", errno);
            return 1;
        }
    }
    int fd = open("/selinux/status", O_RDONLY);
    if (fd < 0) {
        printf("FAIL p1 open status errno=%d\n", errno);
        return 1;
    }
    // THE repro: first fault on a selinuxfs MAP_SHARED mapping.
    volatile unsigned char *p = mmap(NULL, 20, PROT_READ, MAP_SHARED, fd, 0);
    if (p == MAP_FAILED) {
        printf("FAIL p1 mmap errno=%d\n", errno);
        close(fd);
        return 1;
    }
    unsigned char page[20];
    memcpy(page, (const void *)p, 20);   // touches every byte
    int bad = 0;
    for (int i = 1; i < 20; i++)
        if (page[i] != 0)
            bad = 1;
    CHECK(page[0] == 1 && !bad, "p1 selinuxfs status MAP_SHARED fault+read",
          " (version=%u)", page[0]);
    munmap((void *)p, 20);
    close(fd);
    return 0;
}

static int p2_selinuxfs_private(void)
{
    int fd = open("/selinux/status", O_RDONLY);
    if (fd < 0)
        return 1;
    volatile unsigned char *p = mmap(NULL, 20, PROT_READ, MAP_PRIVATE, fd, 0);
    if (p == MAP_FAILED) {
        printf("FAIL p2 mmap errno=%d\n", errno);
        close(fd);
        return 1;
    }
    CHECK(p[0] == 1, "p2 selinuxfs status MAP_PRIVATE fault+read", "");
    munmap((void *)p, 20);
    close(fd);
    return 0;
}

static int p3_procfs_private(void)
{
    if (mount("proc", "/proc", "proc", 0, NULL) != 0 && errno != EBUSY) {
        printf("FAIL p3 mount proc errno=%d\n", errno);
        return 1;
    }
    int fd = open("/proc/meminfo", O_RDONLY);
    if (fd < 0) {
        printf("FAIL p3 open meminfo errno=%d\n", errno);
        return 1;
    }
    // Touch the mapping in a FORKED CHILD: a procfs inode has size 0,
    // so the fault may SIGBUS the child (Linux instead fails the mmap
    // outright with ENODEV — a separate gap). What this case guards is
    // the KERNEL surviving a fault on a foreign-inode VMA (procfs
    // stores pids/pointers in private_data): child exit 0 (read 'M')
    // or SIGBUS death both pass; a KERNPANIC is the regression.
    pid_t pid = fork();
    if (pid == 0) {
        char *p = mmap(NULL, 4096, PROT_READ, MAP_PRIVATE, fd, 0);
        if (p == MAP_FAILED)
            _exit(21);
        _exit(p[0] == 'M' ? 0 : 22);
    }
    int st = 0;
    waitpid(pid, &st, 0);
    int ok = (WIFEXITED(st) && WEXITSTATUS(st) == 0)
             || (WIFSIGNALED(st) && WTERMSIG(st) == SIGBUS);
    CHECK(ok, "p3 procfs MAP_PRIVATE foreign-inode fault (no panic)", " (st=%d)", st);
    close(fd);
    return 0;
}

static int p4_devfs_shared_grow(void)
{
    mkdir("/dev/__parameters__", 0755); // may EEXIST
    const char *path = "/dev/__parameters__/probe_ws";
    unlink(path);
    int fd = open(path, O_CREAT | O_RDWR, 0666);
    if (fd < 0) {
        printf("FAIL p4 open create errno=%d\n", errno);
        return 1;
    }
    if (ftruncate(fd, 0x10000) != 0) {
        printf("FAIL p4 ftruncate 64K errno=%d\n", errno);
        close(fd);
        return 1;
    }
    unsigned char *m = mmap(NULL, 0x10000, PROT_READ | PROT_WRITE,
                            MAP_SHARED, fd, 0);
    if (m == MAP_FAILED) {
        printf("FAIL p4 mmap errno=%d\n", errno);
        close(fd);
        return 1;
    }
    // Init-writer pattern: plain memcpy, no msync.
    for (int i = 0; i < 0x10000; i += 4096)
        m[i] = (unsigned char)(i / 4096);
    // Fault PAST the current shared extent: grow the file and touch a
    // page beyond the old backing (the shared_grow path).
    if (ftruncate(fd, 0x20000) != 0) {
        printf("FAIL p4 ftruncate 128K errno=%d\n", errno);
        close(fd);
        return 1;
    }
    unsigned char *m2 = mmap(NULL, 0x20000, PROT_READ | PROT_WRITE,
                             MAP_SHARED, fd, 0);
    if (m2 == MAP_FAILED) {
        printf("FAIL p4 mmap grown errno=%d\n", errno);
        close(fd);
        return 1;
    }
    m2[0x10000] = 0x10;   // page in the grown region, via the 2nd mapping
    m2[0x18000] = 0xAB;
    CHECK(m2[0x18000] == 0xAB && m2[0] == 0, "p4 devfs shared grow+coherent reread",
          "");

    // Cross-process coherence: child opens independently and must see
    // the parent's stores instantly (tmpfs MAP_SHARED semantics).
    pid_t pid = fork();
    if (pid == 0) {
        int cfd = open(path, O_RDONLY);
        if (cfd < 0)
            _exit(21);
        unsigned char *cm = mmap(NULL, 0x20000, PROT_READ, MAP_SHARED, cfd, 0);
        if (cm == MAP_FAILED)
            _exit(22);
        _exit(cm[0x18000] == 0xAB && cm[0x10000] == (unsigned char)0x10 ? 0 : 23);
    }
    int st = 0;
    waitpid(pid, &st, 0);
    CHECK(WIFEXITED(st) && WEXITSTATUS(st) == 0,
          "p4 child cross-open coherent view", " (st=%d)", st);

    munmap(m, 0x10000);
    munmap(m2, 0x20000);
    close(fd);
    unlink(path);
    return 0;
}

int main(void)
{
    mkdir("/selinux", 0755);
    mkdir("/dev", 0755);   // may EEXIST

    p1_selinuxfs_shared();
    p2_selinuxfs_private();
    p3_procfs_private();
    p4_devfs_shared_grow();

    if (fails == 0)
        printf("SELINUX-MMAP: PASS\n");
    else
        printf("SELINUX-MMAP: FAIL (%d)\n", fails);
    return fails ? 1 : 0;
}
