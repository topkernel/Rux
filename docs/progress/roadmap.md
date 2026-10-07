# Rux Development Roadmap

## Current Phase (2026-10-07): x86_64 lands on main; OpenHarmony program starts

**Milestone: x86_64 is a supported platform on main — Ubuntu 22.04 amd64
boots to the graphical desktop (login screen, keyboard login, apps), the
same gate riscv64 passes.**

### x86_64 bring-up (merged to main)

- Full boot chain on QEMU q35: SeaBIOS/multiboot → 32-bit trampoline →
  long mode → kernel banner → PML4 switch → the whole subsystem table
  (buddy/slab, e820, CFS, softirqs, cgroups, vdso) → virtio-blk/net/gpu
  over PCI → ext4 root → exec → userspace
- Userspace: static binaries, then glibc 2.35 dynamic linking
  (arch_prctl TLS, x86 stat(2) layout, 16-byte-aligned entry stacks,
  orig_rax syscall numbers, NXE, U/S bits on user page-table walks)
- SMP: SIPI AP startup, LAPIC + IPIs, per-CPU GS/swapgs, TLB shootdown
  (4 CPUs online, IPI selftest green)
- The final Ubuntu blocker was a virtio INTx interrupt storm (an
  unregistered device's level-triggered line re-delivered forever,
  starving PID 1 of every syscall) — every virtio PCI function now
  registers in the shared INTx dispatcher at construction
- Build: Linux-style platform selection (`PLATFORM=x86_64 make build`,
  `platform_target` in build/.config, config-overlay semantics)
- Dual-arch regression standard: riscv64 build+smoke must stay green on
  every x86 change and vice versa

### OpenHarmony port program (started)

- Plan: `docs/development/openharmony-port-plan.md` — replace the Linux
  kernel under the OpenHarmony x86_64_virt emulator image with Rux;
  estimated 25-40k new lines across binder (6-10k), graphics (1-8k) and
  the platform port (done)
- Spike S1 (binder): closed loop DONE — cross-process TRANSACTION/REPLY
  with object translation and ref handshake, 325 transactions across 6
  boots, zero leaks (feature/oh-s1-binder)
- Spike S3 (graphics route): fbdev HDI composer route judged viable at
  zero kernel cost (VDI is dlopen-pluggable); requires a GPU-off rebuild
  of the OH graphics stack — see openharmony-s3-fbdev-composer.md
- Phase 1 (initrd boot chain) and Phase 2 prerequisites (ashmem, memfd
  wiring, access_tokenid) in flight on feature branches

## Current Phase (2026-10-04): X11 on Rux → GNOME

**Milestone reached: Xorg runs on Rux — full server initialization, X client
windows render to the screen, and xterm works as an interactive terminal
with a live shell (bash and dash).**

### X11 / desktop bring-up

- Xorg completes full initialization on 3/3 cold boots: screen and all
  extensions, evdev keyboard/mouse devices, InputThread, and the
  `/tmp/.X11-unix/X0` socket. The X client ↔ Xorg request path is fully
  working (`XOpenDisplay(:0)` + `XSync` succeed), and a self-written X
  client's mapped window reaches the display (fbmap probe: 21,384
  non-black pixels, stable across 2 runs)
- The dispatch-path root cause is fixed: `recvmsg` left the caller's
  buffer *capacity* in `msg_controllen` on every success path that
  delivers no cmsgs; Xorg then walked garbage control data whose first
  `cmsg_len` was SIZE_MAX — `CMSG_NXTHDR` never advanced and the dispatch
  thread spun at 100% CPU. `msg_controllen` is now zeroed exactly as
  Linux does
- Input reaches X clients end to end (`test/xinput_probe.c`, raw X11
  protocol, no libX11): QMP-injected KEY_A/KEY_B, absolute pointer moves
  and clicks are delivered as the exact KeyPress/KeyRelease (keycode+8),
  MotionNotify (scaled from the 0..32767 tablet range) and
  ButtonPress/ButtonRelease sequence
- xterm runs as an interactive terminal: it opens its pty through
  `/dev/ptmx` and the shell inside — bash and dash — stays alive. The
  enabling fixes are Linux-exact pty semantics: TIOCGPTPEER (openpty's
  slave acquisition returned a phantom fd 0), the corrected TIOCGPTN
  ioctl number (0x80045430, Linux uapi), TIOCGPGRP with no foreground
  pgrp answering the caller's own pgrp, and ENOTTY for unknown ioctls
- The D-Bus system-bus GNOME blocker is fixed. Filesystem AF_UNIX
  sockets are now keyed by the bound node's inode identity
  (`#<fs_id>:<ino>`), so `/run` and its `/var/run` symlink alias name
  the same listener (GLib hardcoded the alias path and previously got
  ECONNREFUSED against a healthy bus), and a full listen backlog now
  blocks or returns EAGAIN like `unix_wait_for_peer` instead of an
  instant refusal. Verified end to end: gnome-session, gnome-shell,
  gsd-* settings daemons, gnome-keyring-d and dconf-service all stay
  alive through the full session; a 40-minute dbus storm soak
  (3 spawners × 6000 fork/exec) shows zero daemon deaths

### OS completeness

- **Swap is active**: anonymous pages are SwapBacked LRU_INACTIVE_ANON
  members at every mapping site, alloc_pages runs bounded synchronous
  direct reclaim, and `/proc/swaps` reports the 256 MB tail carve. The
  swap-probe gate touches 2,064 MiB (>2 GiB) of anonymous memory and
  verifies 66,048 sampled pages across the swap-out/swap-in cycle;
  teardown frees swap slots back to zero
- **Shutdown cascade end to end**: reboot(2) (incl. CAD_ON/CAD_OFF),
  shutdown(8) (sysvinit sequence), Ctrl-Alt-Del (virtio-keyboard
  modifier tracking + serial CSI encoding), and the PID-1 exit fallback
  all power the machine down through SBI — four scenarios verified with
  clean QEMU exits
- **Network bring-up (P0-2) complete**: AF_PACKET/SOCK_DGRAM cooked
  sockets, AF_INET SOCK_RAW, the full if-ioctls set (netmask, brdaddr,
  dstaddr, metric, txqlen, map, SIOCADDRT/SIOCDELRT), broadcast-MAC
  fast path for 255.255.255.255 and gateway-MAC resolution for off-link
  destinations. busybox `ip`/`ifconfig`/`route` configure the stack,
  `ping` runs 3/3 with 0% loss, and `udhcpc` completes the full
  discover/offer/request/ack cycle with the lease script applied
- **udev/hotplug minimal path**: registration-time `add` uevents over
  NETLINK_KOBJECT_UEVENT with DEVNAME/INTERFACE parity, an
  mdev-compatible `/sys` (block and input class trees, `/sys/dev`
  links), and a PCI ECAM rescan trigger — verified end to end: netlink
  probe → busybox mdev → `/dev/vdb` node created

### Memory leaks closed (root causes, not symptoms)

- **exec/fork page-table leak**: the PT ledger's boot mask stamped every
  memblock-reserved frame, so `free_page_table_checked` silently refused
  every table free — ~100 KB leaked per exec, 2 GB exhausted in ~21 min.
  The boot set is now exact (array, not hash); measured ~0.03 KB/exec
  over 3,500 execs with MemFree flat
- **Task field leak**: `free_task_slot` raw-deallocated the Task page
  without running destructors, leaking `exe_path`, `sem_undo`,
  `dead_threads` and more on every exit — a fork+exec loop grew HeapUsed
  +1,219 kB/min; `drop_in_place` before dealloc makes the watermark
  constant
- Supporting fixes: one-shot timers drop their action entries on expiry,
  `close_cloexec` no longer leaks fds, and the virtio-gpu framebuffer is
  never COW-marked on fork

### Performance and debuggability

- Identity MMIO windows (PCI MMIO 256 MB, ECAM, PLIC) map with 2MB
  superpages: fork+exit −90%, exec −60%, unix ping-pong −43%; TLB
  pressure from kernel-MMIO entries drops from ~68K PTEs to ~132
- printk filters the log level *before* formatting: getpid −81%,
  malloc −63%
- MTTCG (`thread=multi`) remains the default for `make ubuntu-run`:
  boot-to-login median 9.5s → 4.5s (2.1x), variance 3.3s → 0.1s, GUI
  gate 23/23 (`THREAD=single` restores deterministic debugging)
- Core dumps are host-debuggable: `/proc/sys/kernel/core_pattern` with
  specifier expansion, NT_AUXV, byte-exact Linux NT_PRSTATUS/NT_PRPSINFO
  layouts — `gdb --core` reads registers, symbolizes the crash PC and
  walks auxv
- `dfx=memwatch` grew heap + physical-page call-site accounting — the
  instrumentation that grounded every leak hunt above

### LTP

- Six fix rounds so far (~90 files, ~4,000 lines): syscall semantics
  (fadvise, vmsplice, splice, readlink, ioctl EBADF/ENOTTY ordering),
  pty/tty, OFD record locks and file leases, epoll validation, periodic
  timers and sigtimedwait wake, ext4 namei/dentry cache, fallocate,
  mmap/msync/mremap, IPC, network/timer/scheduler batches
- Full-sweep PASS trend across the 1,869-test list: 35 → 524 → 659 →
  677 → 602 (r1–r5). The r5 decline is diagnosed — tmpfs `/tmp`
  exhaustion (93 TBROKs) plus early-exec hangs — and the round-6 batch
  (tmpfs/ext4 space accounting, loop devices, mke2fs-through-loop) is
  landed; the round-6 rescan is running
- The in-kernel unit-test suite was repaired to current semantics:
  985/985 green

**Carried in from the previous milestones (2026-09-29/30):** the complete
Ubuntu 22.04 userland boots — graphical desktop session (framebuffer +
evdev input, pixel-verified 23 checks/boot at `-smp 4`), real command
execution in Ubuntu's own `/bin/dash`, interactive `bash -i`, D-Bus system +
session buses end to end, pty, System V IPC, POSIX MQ, 4-CPU SMP with
cross-core TLB shootdowns on COW, and the boot-time wall clock from the
goldfish RTC. Earlier root causes of the same phase: the TCP timer softirq
allocated from the global heap on every tick even with zero sockets (the
idle livelock), fork CLONE_CHILD_SETTID corrupting the parent TCB (Xorg's
`futex_wait` deadlock), `sys_poll` revents masking and edge-triggered
epoll re-arm semantics (the X client handshake unlocks), and flock/fcntl
parity (24/24 matrix vs Linux).

**In flight (toward full GNOME):**

| Workstream | Status |
|---|---|
| GNOME on screen (gnome-shell on X11 + llvmpipe, openbox fallback) | session survives end to end after the D-Bus fix; on-screen rendering and input acceptance next |
| LTP round-6 rescan | fix batch landed (space accounting, vmsplice/splice/fadvise, loop devices); full sweep running |
| r5 regression families | tmpfs exhaustion fixed; early-exec hang cluster under investigation |

**Next milestones:**

1. GNOME session visible and operable on screen (gnome-shell on X11 + llvmpipe, openbox fallback) with working input
2. LTP round-6 rescan green — recover the 677 PASS baseline and push past it
3. Long-run stability soak (24h boot), performance pass, upstream cleanup

---

## Project Overview

| | |
|---|---|
| **Architecture** | RISC-V 64-bit (RV64GC), 4-CPU SMP |
| **Source Files** | 306 (303 Rust + 3 Assembly) |
| **Code Lines** | ~164,900 |
| **Syscall Numbers** | 345 dispatched |
| **Unit Tests** | 985 cases across 60 test files (985/985 green) |
| **Formal Verification** | 1,088 proptest cases (98 modules), 157 Kani proofs (22 modules), 4 SPIN models (8 LTL), Miri CI |
| **Linux LTP** | 1,838 official tests; full-sweep PASS 35 → 524 → 659 → 677 → 602 (r1–r5), round-6 rescan in progress |
| **Smoke Tests** | 15/15 passing |
| **GUI Gate** | 23 pixel-level checks per boot (login → desktop → apps → real commands) |
| **Boot Mode** | MTTCG default (`thread=multi`): boot-to-login ~4.5s median |
| **Current Phase** | X11 milestone: Xorg, X client windows, interactive xterm; GNOME bring-up |

**Design Philosophy**: External interfaces 100% Linux ABI compatible. Internal implementation free to innovate.

---

## Module Completion

| Status | Modules |
|--------|---------|
| ✅ Complete (9) | Boot & Init · System Calls · Scheduler · Process Mgmt · Security · Diagnostics · Testing · Build & Tooling · Memory Mgmt |
| ⚠️ In Progress (9) | File System 92% · ELF Loader 92% · Interrupts 90% · Synchronization 90% · Block Device 85% · Network 85% · SMP 80% · Exception & Trap 71% · Graphics 70% |

---

## Feature Implementation Status

> ✅ Implemented · ⚠️ Partial · ❌ Not Implemented

| Module | Feature | Status | Feature | Status | Feature | Status |
|--------|---------|--------|---------|--------|---------|--------|
| **1. Boot & Init** ✅ | OpenSBI Integration | ✅ | Assembly Entry | ✅ | MMU Trampoline | ✅ |
| | VMA/LMA Linker Script | ✅ | medany Code Model | ✅ | Stack Setup | ✅ |
| | BSS Zeroing | ✅ | UART (ns16550a) | ✅ | UART Blocking Read | ✅ |
| | TTY ISIG | ✅ | CSR Management | ✅ | sscratch/tp Protocol | ✅ |
| | stimecmp (SSTC) | ✅ | Early Print | ✅ | Boot Page Table (8MB) | ✅ |
| **2. Exception & Trap** ⚠️ | Direct Mode | ✅ | Vectored Mode | ❌ | PtRegs (Linux-style) | ✅ |
| | User/Kernel Stack Switch | ✅ | CSR Save/Restore | ✅ | ecall | ✅ |
| | Page Fault | ✅ | Breakpoint | ✅ | Illegal Instruction | ✅ |
| | FPU Save/Restore | ✅ | FP Exception | ❌ | ret_from_exception | ✅ |
| | ret_from_fork_user | ✅ | ret_from_fork_kernel | ✅ | Signal Frame Delivery | ✅ |
| **3. System Calls** ✅ (348) | File System (93) | ✅ | Process (90) | ✅ | Time & Timer (39) | ✅ |
| | Memory (30) | ✅ | IPC (21) | ✅ | Network (18) | ✅ |
| | I/O (18) | ✅ | Scheduler (15) | ✅ | Misc (14) | ✅ |
| | Signal (9) | ✅ | Diagnostics (1) | ✅ | | |
| **4. Scheduler** ✅ | Scheduling Class | ✅ | CFS v1 (vruntime) | ✅ | Deadline (EDF+CBS) | ✅ |
| | RT FIFO / RR | ✅ | Idle Class | ✅ | Stop Task | ✅ |
| | Global Run Queue | ✅ | CPU Affinity | ✅ | Load Balancing | ✅ |
| | Scheduler Tick | ✅ | Cross-CPU IPI | ✅ | CPU Idle (WFI) | ✅ |
| | POSIX Real-time | ❌ | | | | |
| **7. Memory Mgmt** ⚠️ | Page Descriptor | ✅ | Frame Allocator | ✅ | Memblock | ✅ |
| | Sv39 Page Table | ✅ | PTE Flags | ✅ | Linear Mapping | ✅ |
| | Kernel Mapping | ✅ | MMU Enable | ✅ | Fixmap | ✅ |
| | ASID (9-bit) | ✅ | TLB Flush | ✅ | Huge Page (PMD) | ✅ |
| | Huge Page (PGD) | ✅ | Buddy (MAX_ORDER=10) | ✅ | Zone DMA | ✅ |
| | Zone DMA32 | ✅ | Zone NORMAL | ✅ | Zone MOVABLE | ✅ |
| | Per-CPU Pagesets | ✅ | Slab (10 classes) | ✅ | SlabCache | ✅ |
| | vmemmap | ✅ | pfn_to_page (O(1)) | ✅ | Page Refcount | ✅ |
| | Page Flags | ✅ | mm_struct | ✅ | VMA (BTreeMap) | ✅ |
| | mmap / munmap | ✅ | Fork Address Space | ✅ | copy_kernel_mappings | ✅ |
| | Demand Paging | ✅ | Stack Expansion | ✅ | Guard Page | ❌ |
| | COW Bit | ✅ | Fork COW | ✅ | COW Fault Handler | ✅ |
| | MAP_PRIVATE COW | ✅ | free_user_page_tables | ✅ | AnonVma | ✅ |
| | AnonVmaChain | ✅ | Rmap: Page Fault | ✅ | Rmap: COW | ✅ |
| | Rmap: Fork | ✅ | Rmap: Unmap | ✅ | Rmap: Exec | ✅ |
| | Zone Watermarks | ✅ | LRU (5 lists) | ✅ | kswapd | ✅ |
| | vmscan | ✅ | Page Cache Shrinker | ✅ | try_to_unmap | ✅ |
| | OOM Killer | ✅ | kswapd OOM Escalation | ✅ | /proc/oom_score | ✅ |
| | /proc/oom_score_adj | ✅ | Swap | ✅ | LRU Page Cache | ✅ |
| | /proc/meminfo | ✅ | Page Statistics | ✅ | Page Cache | ✅ |
| ✅ Complete (9) | Boot & Init · System Calls · Scheduler · Process Mgmt · Security · Diagnostics · Testing · Build & Tooling · Memory Mgmt |
| | PID Reuse | ✅ | Kernel Stack Cache | ✅ | Parent-Child-Sibling | ✅ |
| | ListHead | ✅ | Init Process (PID 1) | ✅ | Register Save | ✅ |
| | FPU Save | ✅ | tp Update | ✅ | U-mode Switch | ✅ |
| | User Stack | ✅ | ELF Loading | ✅ | Auxiliary Vector | ✅ |
| | CLONE_VM/FILES/FS | ✅ | CLONE_SIGHAND/THREAD | ✅ | CLONE_SETTLS | ✅ |
| | CLONE_CLEARTID/SETTID | ✅ | CLONE_DETACH | ✅ | robust_list | ✅ |
| | SignalStruct (64) | ✅ | SigAction | ✅ | Signal Mask | ✅ |
| | SIGKILL/SIGSTOP | ✅ | User-mode Handler | ✅ | Signal Frame | ✅ |
| | rt_sigreturn | ✅ | Realtime Queue | ✅ | sigaltstack | ✅ |
| | Signal Edge Cases | ⚠️ | do_exit | ✅ | SIGCHLD | ✅ |
| | Zombie Reaping | ✅ | do_wait | ✅ | FsStruct | ✅ |
| | FdTable | ✅ | brk | ✅ | oom_score_adj | ✅ |
| | Cred (8 IDs) | ✅ | Fork Inheritance | ✅ | kthread_create | ✅ |
| | kthread_should_stop | ✅ | kthread_run | ✅ | | |
| **6. File System** ⚠️ | open/close | ✅ | Path Resolution | ✅ | Symbolic Link | ✅ |
| | Dentry Cache | ✅ | Inode Cache | ✅ | LRU Eviction | ✅ |
| | Superblock | ✅ | Mount/Unmount | ✅ | FdTable (Arc) | ✅ |
| | alloc/install fd | ✅ | fd Reuse | ✅ | O_CLOEXEC | ✅ |
| | Memory FS | ✅ | File/Dir Ops | ✅ | /proc/meminfo | ✅ |
| | /proc/cpuinfo | ✅ | /proc/version | ✅ | /proc/uptime | ✅ |
| | /proc/loadavg | ✅ | /proc/cmdline | ✅ | /proc/mounts | ✅ |
| | /proc/interrupts | ✅ | /proc/self | ✅ | /proc/pid/status | ✅ |
| | /proc/pid/stat | ✅ | /proc/pid/cmdline | ✅ | /proc/pid/exe | ✅ |
| | /proc/pid/cwd | ✅ | /proc/pid/environ | ✅ | /proc/pid/fd | ✅ |
| | /proc/pid/maps | ✅ | /proc/pid/oom_score | ✅ | /proc/pid/oom_score_adj | ✅ |
| | Device Registry | ✅ | /dev/input | ✅ | Circular Buffer | ✅ |
| | Blocking I/O | ✅ | Transaction | ✅ | Commit/Recovery | ✅ |
| | Checkpoint | ✅ | Revoke Records | ✅ | Crash Recovery | ✅ |
| | Superblock | ✅ | Block Group | ✅ | Inode | ✅ |
| | BlockAllocator | ✅ | InodeAllocator | ✅ | mballoc (locality) | ✅ |
| | mballoc (spiral) | ✅ | mballoc (prealloc) | ✅ | Dir/File Ops | ✅ |
| | Extent Tree | ✅ | JBD2 Integration | ✅ | Hard Link | ✅ |
| | Symlink | ✅ | Truncate | ✅ | Rename (renameat2) | ✅ |
| | O_EXCL | ✅ | Shrinker Interface | ✅ | Read-ahead | ✅ |
| | uid/gid Enforcement | ❌ | | | | |
| **9. Interrupts** ⚠️ | PLIC Init | ✅ | Priority/Enable | ✅ | Claim/Complete | ✅ |
| | UART Interrupt | ✅ | VirtIO MMIO | ✅ | VirtIO PCI | ✅ |
| | Interrupt Sharing | ❌ | SBI TIMER | ✅ | SSTC (stimecmp) | ✅ |
| | Periodic Interrupt | ✅ | High-precision Timer | ❌ | SBI IPI | ✅ |
| | Reschedule IPI | ✅ | SSIP + Bitmap | ✅ | | |
| **10. SMP** ⚠️ | SBI HSM | ✅ | Secondary Core Boot | ✅ | Per-CPU Stacks | ✅ |
| | CPU Hot Plug | ❌ | Stack/RunQueue/Idle | ✅ | Pagesets | ✅ |
| | Per-CPU Vars | ❌ | spin::Mutex | ✅ | RwLock | ✅ |
| | SeqLock | ✅ | Kernel Big Lock | ✅ | | |
| **11. Sync Primitives** ⚠️ | Semaphore (down/up) | ✅ | Condvar wait/signal | ✅ | Condvar broadcast | ✅ |
| | wait_timeout | ❌ | Mutex lock/unlock | ✅ | MutexGuard | ✅ |
| | Deadlock Detection | ✅ | Futex wait/wake | ✅ | PI Futex | ✅ |
| | REQUEUE | ✅ | CMP_REQUEUE | ✅ | CLOCK_REALTIME | ✅ |
| | Futex Edge Cases | ⚠️ | Tiny RCU | ✅ | SeqLock | ✅ | | |
| **12. ELF Loader** ⚠️ | ELF Header | ✅ | Program/Section Header | ✅ | Dynamic Linking | ✅ |
| | PT_INTERP | ✅ | Auxiliary Vector | ✅ | Page Table Creation | ✅ |
| | PT_LOAD Mapping | ✅ | VM_EXECUTABLE | ✅ | User Stack/BSS | ✅ |
| | Entry Point/execve | ✅ | Block Sources | ✅ | ASLR/KASLR | ❌ |
| | ld-musl | ✅ | Shebang (#!) | ✅ | | |
| **13. Block Device** ⚠️ | Device Detection | ✅ | VirtQueue | ✅ | Modern PCI | ✅ |
| | VirtIO MMIO | ✅ | Read (MMIO+PCI) | ✅ | Write (MMIO+PCI) | ✅ |
| | Multi-queue | ❌ | BufferHead | ✅ | Block Cache | ✅ |
| | bread/brelse | ✅ | GenDisk | ✅ | Request Queue | ✅ |
| | Request Scheduling | ❌ | | | | |
| **14. Network** ⚠️ | socket/bind/listen | ✅ | accept4/connect | ✅ | send/recv | ✅ |
| | sendmsg/recvmsg | ✅ | shutdown | ✅ | get{sock,peer}name | ✅ |
| | socketpair | ✅ | set/getsockopt | ✅ | Three-way Handshake | ✅ |
| | TCP State Machine | ✅ | Retransmission (RTO) | ✅ | Sliding Window | ✅ |
| | Congestion Control | ✅ | Fast Retransmit | ✅ | TCP Checksum | ✅ |
| | Four-way Close | ✅ | UDP Datagram | ✅ | UDP Checksum | ✅ |
| | IPv4 | ✅ | Routing Table | ✅ | ARP | ✅ |
| | ICMP | ✅ | IP Fragmentation | ❌ | VirtIO-net | ✅ |
| | Packet TX/RX | ✅ | Loopback | ✅ | SkBuff | ✅ |
| | Protocol Layering | ✅ | | | | |
| **15. Graphics** ⚠️ | Framebuffer | ✅ | fbdev | ✅ | VirtIO-GPU | ✅ |
| | GPU Acceleration | ❌ | evdev | ✅ | PS/2 Keyboard/Mouse | ✅ |
| | VirtIO Input | ✅ | Multi-touch/Gamepad | ❌ | rux_gui Library | ✅ |
| | Desktop/Calculator | ✅ | Clock/vshell | ✅ | Full Desktop | ❌ |
| **16. Diagnostics** ✅ | Panic Handler | ✅ | Stack Trace | ✅ | Hung Task Detector | ✅ |
| | printk | ✅ | Ring Buffer | ✅ | pr_* Macros | ✅ |
| | errno (50+) | ✅ | Result/Option | ✅ | | |
| **17. Testing** ✅ | Framework (60 files) | ✅ | ListHead/Path/FileFlags | ✅ | Heap/PageAlloc/COW | ✅ |
| | Scheduler/Signal | ✅ | fork/execve/wait4 | ✅ | file_open/FdTable | ✅ |
| | Dcache/Icache/ext4 | ✅ | virtio_queue | ✅ | Boot/Multicore | ✅ |
| | mini-lTP (25) | ✅ | Smoke Tests (15/15) | ✅ | | |
| **18. Build & Tooling** ✅ | Cargo Workspace | ✅ | Makefile | ✅ | QEMU Scripts | ✅ |
| | Kernel.toml | ✅ | menuconfig | ✅ | test/run.sh | ✅ |
| | README | ✅ | Architecture Docs | ✅ | Design/Dev Guides | ✅ |
| **19. Security** ✅ | Cap Type (u64) | ✅ | 41 CAP_* Constants | ✅ | capable() API | ✅ |
| | capget / capset | ✅ | Signal Permission | ✅ | File Permission | ✅ |
| | IPC Permission | ✅ | setuid/setgid Exec | ✅ | LSM Hook Framework | ✅ |
| | Capability LSM | ✅ | euid→cap Migration | ✅ | | |
---

## Development History

| Era | Phase | Theme | Key Deliverables |
|-----|-------|-------|-----------------|
| Foundation | 1–5 | Boot & Basics | OpenSBI, MMU trampoline, exceptions, buddy allocator, fork/execve, scheduler |
| Core Infra | 6–10 | Interrupts, SMP, Sync | PLIC/timer/IPI, 4-core HSM boot, spinlock/RwLock/mutex/condvar, VFS, ELF |
| User Mode | 11–15 | Signals, COW, Testing | U-mode switch, signal frame, clone flags, COW, pipe, 60 test files, mini-lTP |
| Storage | 16–17 | Block Device & Filesystem | Preemptive sched, VirtIO-blk, ext4 (inode, dir, extent tree), bio cache |
| Network | 18 | TCP/IP Stack | SkBuff, ARP, IPv4, UDP, TCP (handshake, state machine, retransmission), VirtIO-net |
| Platform | 18.5–22 | Modernization & Shell | VirtIO PCI 1.0+, musl toolchain, toybox, multi-shell, procfs, boot beautification |
| Scheduler | 23–25 | CFS & Reliability | CFS v1 (enabled by default), COW (fork+mmap), TCP retransmission, sigaltstack |
| Hardening | 26–28 | Linux-Style Arch | Zone allocator, vmemmap, PCP, memblock, ASID, rmap, MMU trampoline, FPU, JBD2 |
| Audit | 29–32 | Correctness & ABI | ext4 write correctness, 345 syscall audit (6 fixes), VFS path cleanup, concurrent I/O |
| Refactoring | 33–36 | VFS & FS Cleanup | inode.ops unification (-44% VFS code), JBD2 recovery, mballoc, async I/O |
| IPC | 37–38 | Inter-Process Communication | System V IPC (sem/msg/shm), POSIX MQ, 18 syscalls, 6 rounds correctness fixes |
| Memory Safety | 39–40 | Rmap & OOM | try_to_unmap (task scan), OOM killer (oom_badness, SIGKILL, kswapd escalation) |
| Security | 41 | Capabilities & LSM | POSIX.1e caps (41 CAP_*), capget/capset, LSM framework, signal/file/IPC permission, setuid/setgid exec |
| Networking | 42 | TCP Close & ICMP | TCP four-way close (FIN/RST/process_ack), ICMP echo reply, dest unreach, tcp_v4_err |
| Memory | 43 | Swap | Swap entry encoding (PTE bit 62), swap device (bitmap slot allocator, VirtIO-blk), swap-out (vmscan→swap_write→unmap_with_swap), swap-in (page fault→swap_read→map), LRU/rmap field conflict resolved (dedicated lru_next) |
| Async I/O | 44 | IO_uring | io_uring_setup/enter/register (NR 425-427), SQ/CQ ring buffers (mmap shared), opcodes: NOP/READ/WRITE/FSYNC/CLOSE/FADVISE, eventfd notification, Linux ABI compatible wire format |
| Memory | 45 | LRU Page Cache | Page cache pages on LRU_INACTIVE_FILE, LRU-based eviction (access-recency), Referenced flag for active/inactive rotation, /proc/meminfo real Cached/Active(file)/Inactive(file)/Swap stats |
| Timers | 46 | POSIX Timers | Timer wheel (BTreeMap + Hrtimer softirq), setitimer/getitimer (ITIMER_REAL with SIGALRM), timer_create/settime/gettime/delete/getoverrun, timerfd_create/settime/gettime (read returns expiration count), periodic timer re-arm |
| FS | 47 | JBD2 Crash Recovery | Two-pass recovery (PASS_SCAN finds last valid commit block, PASS_REPLAY replays only committed transactions), prevents replaying incomplete transaction data after crash |
| Sync | 48 | Tiny RCU | Non-preemptible RCU (rcu_read_lock = preempt_disable), per-CPU callback lists, softirq-driven callback processing, generation-counter grace period detection, QS hooks in __schedule and cpu_idle_loop, boot.S early page table expanded 4MB→8MB |
| Sync | 49 | RCU PID Hash Table | PID hash table rewritten from BTreeMap to RCU-protected chained hash table, lock-free lookup via rcu_read_lock/unlock, per-bucket spinlock for insert/remove, synchronize_rcu in release_task for safe deferred reclamation |
| Sync | 50 | SeqLock | Sequence lock for read-mostly data (RawSeqLock + SeqLock<T: Copy> + SeqLockWriteGuard), lock-free readers with retry-on-write, writer serialization via odd/even sequence counter, loopback/hugepage stats converted from Spinlock |
| Memory | 51 | Memory Compaction | Two-pointer scan compaction (migrate UP + free DOWN), page migration (unmap + copy + remap), compaction fallback in alloc_pages for high-order allocations, free block consolidation via buddy merge |
| Hardening | 52 | Process Exit Race & Defensive Checks | Deferred exit notification (do_exit stores parent PID in per-CPU slot, __schedule processes it after context switch to prevent use-after-free), defensive ti_cpu bounds checks in trap.S and cpu_id() (clamp -1/invalid to CPU 0), bio lock ordering fix, rootfs hard link Arc<Vec> COW fix, TCP reliability (SYN-ACK retransmit, FIN drain, seq wrap), CFS Vec len vs capacity |
| Desktop | 53–54 | Ubuntu Userland & X11 | Ubuntu 22.04 desktop (framebuffer + evdev, GUI gate 23/23), D-Bus, MTTCG default (2.1x boot), goldfish RTC wall clock, Xorg full init + X client window rendering, interactive xterm (pty semantics), GNOME session survives (unix-socket inode keys + backlog semantics) |
| Completeness | 55 | Lifecycle, Swap, Network Bring-up | reboot(2)/shutdown(8)/Ctrl-Alt-Del cascade, swap activation (>2 GiB verified), AF_PACKET/raw sockets + udhcpc end to end, udev/hotplug (uevent + mdev /sys), host-debuggable core dumps, page-table and Task-field leak root causes, MMIO superpages + printk fast filter, LTP r1–r5 sweeps (PASS 35 → 602, six fix rounds) |

---

## Planned Features

| Priority | Feature | Description |
|----------|---------|-------------|
| P1 | PID namespace | Process isolation |
| P1 | cgroup v1 | Basic resource control (memory, CPU) |
| P1 | IP fragmentation | Jumbo frame support |
| P2 | Transparent huge pages | PMD fault handler integration |
| P2 | Device tree (DTB) | Hardware description parsing |
| P2 | Vectored trap mode | Faster interrupt dispatch |
| P3 | Virtualization | KVM, containers |
| P3 | Power management | Frequency scaling, hibernate |
| P3 | Multimedia | Audio, video |
| P3 | CPU hot plug | Runtime CPU add/remove |
| P3 | ASLR / KASLR | Address space layout randomization |
| P3 | POSIX real-time | Full real-time scheduling support |
| P3 | File capabilities | security.capability xattr support in ext4 |
| P3 | MAC module (Smack/SELinux) | Mandatory access control via LSM framework |

---

**Document Version**: v31.0
**Last Updated**: 2026-10-04
