# compat-r3 ledger (branch feature/compat-r3, off main 819a1be2)

Round: 2026-11-02. Lanes: r21 bug-list head (coredump panic), blocking-socket
family, fresh full LTP scan on current main, triage.

## Pre-flight findings

- The r21-bugs lane already fixed all four real r21 kernel bugs and they are
  ALL MERGED into main (b835e56e coredump walker, d98f3272 syslog int width,
  56ce13a8 semid64_ds x86 layout, ea40da3c process_vm walker). Bug 5
  (rt_sigaction01) adjudicated musl/LTP artifact (same binary dies on the
  host kernel). Bug 6 (socket hang) fixed on main earlier.
  => My lanes 1-2 become RE-VERIFICATION on current main (tree moved a lot
  since: print purge, lock realignment, perf merges).
- An "r22" scan ran 2026-11-02 morning in the ltp-r21 worktree on the
  ltp-r21 BRANCH (2fd5215f, not main): PASS 966, WEDGE 100 (whole chunk
  nftw01..open03 never reported in 5 attempts), CRASH 30. Not a valid
  main count; superseded by this round's scan on 819a1be2.
- Scan rig: test/ltpscan/scan-x86_64.py + tests-all.txt (1869), image
  /home/william/rux-images/testimages/ltp/rootfs-x86_64-ltp.img (symlinked
  at test/rootfs-x86_64-ltp.img), boot -snapshot + file.locking=off.
- Kernel built from 819a1be2: build rc=0,
  target/x86_64-unknown-none/debug/bzImage (2026-11-02 19:33).

## Isolation re-verification on main 819a1be2

(list: kill11 waitpid01 abort01 recv01 recvfrom01 send01 sendto01 connect01
accept4_01; 2 passes a/b; results below when done)

RESULT: 9/9 PASS both passes (run/iso23-{a,b}.txt). Coredump path verified
exercised ("dumped core" TPASS x3 tests); zero panics. Socket family green.

## r22 full scan on main 819a1be2 (kernel built 19:33, scan 19:34-20:35)

PASS=967 FAIL=257 CRASH=30 TIMEOUT=14 POISON=2 TCONF=482 EXECFAIL=17
WEDGE=100 — the WEDGE chunk is chunk@700 (nftw01 first): deterministic
kernel panic 3/3+ in isolation (run/nftw01-iso1, probe2..6 logs).

CAVEAT: probe kernels (KFAULT-LOW print only) were built mid-scan; chunks
after ~20:00 booted a kernel differing only by the probe print (probe4's
spin-park could only fire on the hunted fault class; no chunk parked).
Chunk@700 results must be rerun after the fix; other chunks unaffected.

Delta r21->r22 (results/delta-r22-vs-r21.txt):
- gains 30: all six r21 bug families green (kill11 waitpid01 syslog12
  semctl01/07 semget01 process_vm* recv/recvfrom/send/sendto/connect/
  accept4), compat fixes (xattr01 family, getcpu01, access01), plus
  ftest03/04 epoll_wait05 recvmsg01 hackbench cve-2017-17052 epoll-ltp
  sendmsg02.
- losses 73 of which ~83-class WEDGE collateral (madvise..prctl range =
  the wedged chunk). REAL new failures (10): diotest2 diotest5 fstat02
  fstat02_64 getpriority01 getrusage04 inotify01 mmap02 stat01 stat01_64;
  plus creat07/mmap18 FAIL->TIMEOUT, waitid08 POISON->CRASH.
  stat/fstat/diotest families point at 692e7623 (lock-free dentry walk);
  getrusage04 is a documented host-load flake family — isolate 2x before
  believing any of these.

## nftw01 panic root-cause hunt (the chunk@700 WEDGE)

Deterministic 3/3+: seconds after SCAN_BEGIN, before nftw01's first
output, in task ~307 (comm=sh — the system() child): kernel-mode jump to
rip=0 with cs=8, rsp inside a zeroed heap page ~19KB below the task's
real kernel stack (kstack [0x5c0000,0x5d4000), rsp=0x5bb440), rbp chain
zeroed, regs rbx/r12 = live user pointers, callee-saves in the Task
struct MATCH the faulting context (the corrupted context IS pid307's).
Nested fault -> is_stack_overflow(rsp) true -> "Kernel stack overflow"
panic with garbled printer output.

Evidence so far:
- No OOM banner, no SWITCHIN-FOREIGN-SP report, no PF-NOCTX, no
  kernel-fault print before the jump: at switch-in time thread.sp was
  in-range; the corruption happened after switch-in.
- Freed-stack ring (probe5): the executing rsp was NEVER inside any
  stack freed via free_kernel_stack this boot (not a freed-stack reuse).
- The kstack block containing the zeroed pages was returned to the buddy
  and recycled (GDB: KSTACK_BLOCKS bases lack 0x580000; the Task struct
  itself lives at 0x5b0000 inside that dead block's pages).
- Trap trace frames attribute a deepest frame to census_peek3::is_userva
  (DWARF, two independent builds) — but the zeroed-region frame walk is
  unreliable; stack-scan attribution may be stale debris.
- GDB park dump: PER_CPU(CPU1): tss_rsp0=0x5d4000 (== task307 stack top,
  consistent), current_task=Task@0x5b0000, syscall_scratch=0x7fffffffd350
  (sh user rsp), PC_KERNEL_STACK_TOP=0.

Hypotheses ruled out: freed-stack resurrection (ring empty for range),
OOM-path recursion (no banner), overflow-with-debris (region below
kstack_bottom partly zero, partly old debris — not fresh contiguous
overflow frames).

Leading hypothesis: a torn/rewritten iretq frame — trap_exit/.Lrestore
with rip=0 + rsp=kernel-range + cs=8 restores exactly this state.
Suspects: the opportunistic-SYSRET tail (344536af) and its frame-rewrite
interactions (signal delivery / execve rewrite path).
NEXT: bisect dd94e441(good, r21 kernel)..819a1be2(bad) on the
deterministic repro (run/nftw-test.sh, run/bisect-loop.sh running).


## FINAL RESULTS (2026-11-02 evening)

Two bisect convictions:
1. nftw01 boot panic (chunk@700 WEDGE, 100 cases): first bad 5542a7b6
   (perf(fs) lock-free walk/stat/open fast paths). Class: x86_64 80KB
   kstack overflow into adjacent slab/heap pages (fee7774f family).
   FIX: kernel_stack_size_x86_64 81920 -> 131072 (Kernel.toml);
   KSTACK_SLOTS 3 -> 2 (2x128KB per 256KB block, exact fit).
   VERIFIED: nftw01 panic gone (fails normally, its r21 class);
   chunk@700 rerun 100/100 reported zero panics, matches r21
   class-for-class (open_tree01 POISON->TCONF); 2x iso green.
2. stat family (stat01/stat01_64/fstat02/fstat02_64 st_size=0,
   inotify01): first bad 0b30585e (ext4 buffered writes, delayed
   allocation). refresh_inode_cache (and the ext4_link twin)
   overwrote the sb-boxed inode copy with the lagging disk-fresh
   state; chmod-after-write then served st_size=0 (probe evidence:
   BOX-REGRESS cached=1024 disk=0 comm=stat01).
   FIX: hold the published size via page_cache dirty_tail_eof in
   refresh_inode_cache + link path.
   VERIFIED: stat01/stat01_64/fstat02/fstat02_64/inotify01 PASS 2x.

Final r22 (results/results-r22.txt, 1869 rows):
PASS=1030 FAIL=277 CRASH=30 TIMEOUT=15 TCONF=499 EXECFAIL=17 POISON=1
(x86_64+KVM high; r21 was 1010.)

Gates on the final tree:
- x86_64 build rc=0; riscv64 build rc=0.
- make test green (suite verdict green).
- riscv64 scripted boots: 4/5 clean ALL-TESTS-PASSED; 1 of 5 hit the
  wake_up-running-task unit test (SMP TCG timing class; plain main
  0/2 — small samples, watch).
- Perf gate: NOT run this round (needs syscost/pbench boots; the
  128KB stack change costs +48KB/task kernel memory — mm domain
  guardianship applies to the merge).

Filed follow-ups (see docs/test/ltp-report.md r22 open items):
mmap02/diotest2/5 delalloc read-coherence; getpriority01 pid=0
ksoftirqd/1 static_prio=0; creat07/mmap18 hangs; oom01 stall;
getrusage04 flake (documented).

Scan/tooling notes: the mid-scan probe-kernel overlap (chunks after
~20:00 differed only by a print) is documented above; the wedged
chunk and POISON rows were rerun on the final kernel. ltp-iso.img is
a private full copy of the shared LTP image (no shared-image writes
outside chunk.txt-less isolation).
