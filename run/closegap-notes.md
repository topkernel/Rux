# closegap lane notes — worst lmbench rows (feature/perf-closegap)

Branch `feature/perf-closegap` off main e6401a4a. Rig: x86_64 + KVM,
strong profile, q35 `-smp 2 -m 2G`, image `work/lmbench-x86.img`
(reflink copy of the shared lmbench x86 image), one case per boot
(`userspace/lmbench/run-rux.sh` pattern; per-case script
`run/cg-cases.sh`, A/B `run/cg-ab.sh`).

Targets (fresh sweep, Rux-KVM vs reference-native,
lmbench-full2.txt in code-review-perf/run):

| Case | Rux | Ref | ratio |
|---|---|---|---|
| lat_sem | 5.34 us | 0.444 | 12.0x |
| lat_unix | 11.80 us | 3.01 | 3.92x |
| lat_ctx 2/8 | 4.62/4.46 | 0.91/1.3 | 5.1/3.4x |
| lat_pipe | 9.05 us | 2.74 | 3.30x |
| open | 1.128 us | 0.41 | 2.75x |

## 1. Attribution — cctx probe (kernel/src/dfx/cctx.rs, cmdline `cctx=1`)

NS-sum phase brackets + counts, dumped from the shutdown cascade (the
lmbench rig ends every boot with reboot(2)). Brackets live in
__schedule, arch context_switch (per-CPU SW_STAMP hands the pre-asm
stamp to the incoming task), Task::wake_up_impl, resched_cpu, and the
semop/pipe/unix bodies. Probe cost when off: one relaxed load.

lat_sem boot (load ~6.4, numbers are host-preemption-inflated but the
ranking holds):

- __schedule phases (per entry): guards 36, GRQ lock-wait 41, acct 47,
  prev deq/requeue 93, pick 248, publish 21 ns.
- context_switch (per switch): fpu save 238, CR3 543 (EVERY switch —
  PCID off), MSR rd/wr pair 393, asm 463, fpu restore 242 ns.
- wake path: select 29, locked enqueue 178, dispatch 937 ns; IPI send
  935 ns avg — 825K IPIs for 851K wakes (every handoff IPIs the idle
  CPU of the ping-pong).
- semop: prelude 318 ns; try_apply_semops 728 ns per call x 2.37M
  (heap Vec x2 + [i32;500] memset + undo-lock + 3 redundant SEM_IDS
  lock pairs per op).

lat_pipe boot (quieter window): machinery cheaper (fpusave 78, mm 138,
msr 121, asm 146, fpurest 104) — the sem boot's numbers were inflated
by host preemption inside brackets; pipe write avg 1740 ns (of which
~1350 = wake dispatch incl. IPI), pipe read avg 7308 (includes the
blocked wait). The uncovered remainder of the 9-10.8 us roundtrip is
the wake->run latency: IPI delivery + halt-exit + idle-loop pass on
the target CPU (exit latency under KVM).

Conclusion: the four latency rows share two cost centers —
(a) the idle-handoff IPI + halt-exit path per context switch,
(b) per-op bodies (semop apply, unix/pipe op + wake dispatch).

## 2. Fixes

1. **semop fast path** (ipc/sysv_sem.rs): try_apply_semops is
   allocation-free for nsops<=8 (stack scratch `[i32;8]` result array +
   `[u16;8]/[i32;8]` per-distinct-sem accumulation; larger sets keep
   the classic Vec working copy), skips the orig_vals Vec and the task
   undo-table lock pair when no op carries SEM_UNDO;
   sys_semtimedop stages sops in a [SemBuf;8] stack array for
   nsops<=8 (was a [SemBuf;500] = 3KB memset per call) and resolves
   id/seq/perms/nsems in ONE SEM_IDS slots-lock section (was 3 sections
   / 6 lock pairs).
2. **Polling idle + IPI skip** (sched/sched.rs): the idle loop spins
   30 us (checking NEED_RESCHED / grq nr_running) before halting, with
   a per-CPU POLLING_IDLE flag; resched_cpu skips the resched IPI while
   the target is polling. Dekker pairing: SeqCst fences on both sides
   (waker: NEED_RESCHED store -> fence -> flag load; idler: flag clear
   -> fence -> final recheck) close the store-buffering race.
   Ping-pong handoffs (lat_pipe/ctx/sem/unix) never IPI again; the
   idle CPU observes the work in tens of ns instead of the
   IPI+halt-exit exit latency.
3. **FPU XSAVEOPT** (arch/x86_64/thread.rs): fpu_save_for_switch uses
   xsaveopt64 when CPUID.0xD.1:EAX[0] — the save skips writing
   components the task did not touch since its xrstor switch-in.
4. **FS/GS MSR cache** (arch/x86_64/context.rs, process.rs): the
   switch-out no longer rdmsr's FS_BASE/KERNEL_GS_BASE —
   thread.fs_base/gs_base are the authoritative mirrors (every writer
   audited: arch_prctl SET_FS/SET_GS, flush_thread, smp bring-up; SET_GS
   now also programs the SWAPGS shadow so the mirror is exact), and the
   two wrmsr are compare-and-skip (common case: same TLS pair skips
   both). Also fixes a latent bug: the old switch-out rdmsr overwrote
   thread.gs_base with the stale shadow MSR, silently losing a pending
   ARCH_SET_GS value at the next context switch.
5. **Idle-loop UART poll gating** (sched.rs + WaitQueueHead::
   has_sleepers): the pre-halt uart_has_data() port read (a KVM exit)
   runs only when a console reader is parked on read_waitq.
6. **Close-side record-lock fast path** (fs/locks.rs): RECORD_LOCK_COUNT
   mirror (stored inside every table critical section); both per-close
   release paths (posix_release_on_close via displaced_close_decision,
   ofd_release_file via File::drop) skip the global table lock + retain
   scan when the box holds no record locks. This was the remaining
   perf-path close-side lever (~40-80 ns of the 300-400 ns close).

## 3. Filed, not done (perf-path lane's bigger step)

- **lookup_child RCU**: per-component BTreeMap-under-spinlock ~39 ns
  per dcache hit (fsw WALK_B_NS). Needs an RCU/seqlock read side for
  the dentry child map — design step, not attempted in this lane.
- The walk-start VFS_STATE item from perf-path notes is ALREADY on main
  (review-s2b's VFS_ROOT_PINNED lock-free root read in get_vfs_root).

## 4. Results

(see run/ab-fix2.txt, interleaved base/fixed boots, med-of-runs)

## 5. Gates

- dual-arch build: make build && make build-x86 rc=0
- make test (riscv64): run/maketest-fix2.log
