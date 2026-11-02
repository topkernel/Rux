# sec-r1 — security round notes (2026-11-02)

Worktree: /home/william/rux-agents/sec-r1 (feature/sec-r1, off main 819a1be2).
Design doc order: docs/architecture/security-design.md §5.

## Lane 1 — capability shaping (L3)

Full audit of all 41 capabilities in kernel/src/security/capability.rs
against every `crate::security::capable` / `has_capability` call site
(note: the design doc says "43"; the file defines 41, CAP_CHOWN(0) ..
CAP_CHECKPOINT_RESTORE(40) — UAPI-complete).

- Consulted before this round (23): CHOWN, DAC_OVERRIDE, FOWNER, FSETID,
  KILL (can_send_signal), SETGID, SETUID, SETPCAP, LINUX_IMMUTABLE,
  NET_BIND_SERVICE, IPC_LOCK, IPC_OWNER, SYS_MODULE, SYS_CHROOT,
  SYS_PTRACE, SYS_ADMIN, SYS_BOOT, SYS_NICE, SYS_RESOURCE, SYS_TIME,
  SYS_TTY_CONFIG, MKNOD, SYSLOG.
- Never checked (18) → triaged:
  - Wired this round (6): NET_RAW, NET_ADMIN, SYS_PACCT, SETFCAP,
    PERFMON, DAC_READ_SEARCH.
  - No live syscall surface, nothing to wire (12): NET_BROADCAST (the
    reference does not gate SO_BROADCAST either), SYS_RAWIO (no
    ioperm/iopl//dev/mem), LEASE→wired? no — see below, AUDIT_WRITE /
    AUDIT_CONTROL / AUDIT_READ (no audit subsystem), MAC_OVERRIDE /
    MAC_ADMIN (no MAC; selinux xattr reads return a default context),
    WAKE_ALARM / BLOCK_SUSPEND (no *_ALARM clock ids, no EPOLLWAKEUP),
    BPF (sys_bpf ENOSYS), CHECKPOINT_RESTORE (pidfd_getfd ENOSYS).

Wiring (reference semantics pinned from the v6.1 sources):

1. net/raw.rs — packet_socket_create: CAP_NET_RAW FIRST (packet_create
   order: EPERM beats the type check); raw_socket_create: CAP_NET_RAW
   AFTER the protocol filter (inet_create order: EPROTONOSUPPORT for
   protocol 0 beats EPERM).
2. net/netlink.rs — SIOCSIF*/SIOCDIFADDR/SIOCADDRT/SIOCDELRT if-ioctls:
   CAP_NET_ADMIN after the request copy, before the device lookup
   (EPERM beats ENODEV); rtnetlink RTM_NEWLINK/DELLINK/NEWADDR/DELADDR/
   NEWROUTE/DELROUTE: CAP_NET_ADMIN at arm entry (EPERM beats the
   EOPNOTSUPP/EINVAL answers).
3. syscall/file.rs — acct: euid!=0 replaced with capable(CAP_SYS_PACCT).
4. fs/xattr.rs — security.capability set/remove: CAP_SETFCAP
   (cap_inode_setxattr/removexattr parity). Other security.* names keep
   CAP_SYS_ADMIN — DELIBERATE DIVERGENCE (register candidate): the port
   answers security.selinux reads with a default context for
   label-carrying images; root-driven restore-time label writes must
   keep working. The reference with no SELinux loaded would EPERM them.
5. fs/permission.rs — CAP_DAC_READ_SEARCH on directories for masks with
   no write bit (generic_permission's second cap branch).
6. perf/mod.rs — !exclude_kernel events require CAP_PERFMON or
   CAP_SYS_ADMIN, else EACCES (perfmon_capable at the default
   perf_event_paranoid=2); exclude_kernel events stay unprivileged.
   Filed for later: LEASE (fs/locks.rs set_lease lacks the reference's
   fsuid==owner||CAP_LEASE gate — deferred: LTP lease rows run as root
   and the gate needs a deliberate pass over fcntl33-style precedence).

A/B: test/caps_probe.c + test/sec-r1-smoke.sh — 16/16 rows PASS on
riscv64 (root paths keep working, nobody gets EPERM/EACCES, errno
precedence preserved). Serial log: run/sec-r1-smoke-serial.log.

## Lane 2 — uaccess TOCTOU (L2)

Swept every syscall that performs ≥2 user copies (58 functions) against
the filed shape (validate-then-re-fetch). Prior work (review-findings-*
ledgers) already covered sockaddr clamps, futex-style shared-memory
reads (by design), and process_vm_*/capset/seccomp/poll/select families
were already single-fetch (kernel snapshot) — nothing to redo there.

Found and fixed (syscall/network.rs):
- sys_recvmsg: the iovec array was fetched once for validation (access_ok
  + aggregate bound) and then RE-FETCHED in four scatter loops (unix,
  udp/tcp, netlink, raw families) — a racing thread of the same process
  could swap iov_base/iov_len between validation and use, redirecting
  the copy_to_user scatter to unvalidated addresses (classic
  double-fetch; self-corruption class — copy_to_user's internal
  access_ok keeps it out of kernel space). Fixed: one kernel snapshot
  Vec<(base,len)> built during validation; every scatter loop consumes
  the snapshot.
- sys_recvmmsg: same shape per message (validate loop + scatter loop
  re-fetch). Fixed identically.
- sendmsg/sendmmsg/writev/readv: already gather/scatter-as-you-go on a
  single fetch per entry — no change.
- sys_poll: re-fetches the pollfd array every wait round but each round
  operates on that round's snapshot (no cross-copy validation) — noted,
  no fix needed for the TOCTOU class.

A/B: behavior-preserving refactor; regression battery
test/sockaddr_clamp (x86_64 KVM, recvmsg/namelen/truncation rows) all
PASS post-change; make test suite green.

## Lane 3 — W^X (L4/posture)

State found:
- ELF load segments: already per-p_flags (RX/RW/RWX) with per-segment
  PTE tightening on both arches (x86_64 sets NX for non-exec segments,
  EFER.NXE armed).
- mmap: exact prots (no implicit W for exec).
- PT_GNU_STACK: NEVER consulted — stack always NX (secure default but
  non-compliant: binaries requesting an executable stack would crash).
- READ_IMPLIES_EXEC personality: stored by personality(2) but ignored by
  mmap.

Reference semantics pinned (fs/binfmt_elf.c, mm/mmap.c, per-arch
elf_read_implies_exec): on BOTH Rux arches a missing PT_GNU_STACK does
NOT set READ_IMPLIES_EXEC (x86_64: mmap_is_ia32() false; riscv64: the
include/linux/elf.h fallback is 0) — only 32-bit x86 binaries get it
automatically.

Fixed:
- process/exec.rs + fs/elf.rs: PT_GNU_STACK parsed; PF_X → stack mapped
  RWX and the stack VMA carries EXEC (EXSTACK_ENABLE_X); anything else
  or missing → NX. Arch-conditional stack PTE encoding: riscv64 exec =
  X bit; x86_64/aarch64 exec = NX/UXN CLEAR — the x86_64 initial stack
  previously had NO NX bit, i.e. it was executable-by-absence (now NX).
- syscall/memory.rs: READ_IMPLIES_EXEC (0x0400000, include/uapi
  personality.h — first attempt used 0x04000000, caught by the probe)
  makes PROT_READ mmaps PROT_EXEC, matching do_mmap. The noexec-mount
  carve-out is not implemented (mmap does not consult mount noexec for
  exec mappings at all today) — filed below.
- mprotect: no restriction added (the reference allows W^X violations
  via mprotect; the posture goal is the load-time default).

A/B: test/wx_probe.c built twice (default / -z execstack): default
build faults stack fetch (SIGSEGV) and execstack build runs stack code;
mprotect RW→RX JIT shape runs; PROT_READ file mapping NX baseline,
becomes executable under READ_IMPLIES_EXEC. 6/6 rows PASS per variant.

Filed (posture ledger candidates):
- mmap ignores noexec mounts for PROT_EXEC file mappings (affects the
  READ_IMPLIES_EXEC carve-out and plain exec mappings alike).
- Kernel-image W^X (riscv64 whole image RWX; x86_64 image RW) remains
  the posture ledger's PROJECT row — untouched this round.

## Lane 4 — SMAP boot assertion (G1 follow-up)

The CPUID fix (EBX[20]) was already on main. Deliverable here: the boot
log must state the REAL armed state from the register read.

Finding while wiring it (IMPORTANT, needs its own lane):
- Actually arming CR4.SMAP triple-faults the machine IMMEDIATELY after
  the CR4 store, on BOTH KVM and TCG (q35, -cpu max): the first kernel
  data access following the arm faults with no IDT in place. Evidence:
  cpuid ebx=0xf1bb07ab (SMAP present), pre-arm live CR4=0x50620, print
  between the store and the next statement never appears; QEMU exits on
  reset (-no-reboot), -d cpu_reset shows the triple reset.
- Because the pre-fix CPUID read EDX[20] (never set on any box we run),
  the arm path had NEVER executed anywhere — the G1 defect did not just
  leave SMAP off on silicon, it masked this latent early-boot U/S-deref
  bug entirely. Prime suspect: a kernel stack or per-CPU structure
  mapped with the user bit.
- Consequence for this round: enable_smap() (the arming) is NOT called
  on boot; cpu::smap_report() prints the live state instead —
  "arch: WARNING — CR4.SMAP NOT armed though CPUID.7.0:EBX[20]=1 ...
  ABSENT on this boot" — so the gap is loud on every boot (the actual
  deliverable). SMAP_ENABLED stays false, keeping the fault classifier
  honest.
- Follow-up work item: audit x86_64 kernel stack / per-CPU mappings for
  stray U/S bits (mmu_init kernel_flags, boot.S table wiring), fix, then
  re-add the arm call inside fpu_init (per-CPU) with the read-back
  verification kept.

## Gates

- make build (riscv64) rc=0; make build-x86 rc=0.
- make test (riscv64 suite): green.
- test/sec-r1-smoke.sh: ALL GREEN (caps 16/16, wx 6/6 per variant).
- test/sockaddr_clamp_boot.py --arch x86_64 --accel kvm: SOCKADDR-PASS.
- scripts/review_coverage.py --check: pass (run after ledger update).

## Commit map

1. sec(l3): capability gates — raw.rs, netlink.rs, file.rs, xattr.rs,
   permission.rs, perf/mod.rs + caps_probe + smoke driver.
2. sec(l2): network.rs recvmsg/recvmmsg snapshot.
3. sec(l4): W^X — exec.rs, elf.rs, memory.rs + wx_probe + tarball repack.
4. sec(x86_64): cpu.rs, mod.rs, security/mod.rs — smap_report.
