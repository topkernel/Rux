Lane: LTP r20 full scan (riscv64)
Baseline r19 = 1,018/1,869. Work autonomously; never ask. Worktree = THIS (main@82059759; test/rootfs.img may need symlink -> /home/william/workspace/Rux/test/rootfs.img).

---

# LTP r22 SESSION 2 (2026-11-02 evening, second successor; branch ltp-r21 @ main 819a1be2 + 2 commits)

## CHUNK@700 PANIC: DECODED AND FIXED (commit 58b27dc4)
The deterministic boot panic (4/4 raw-scan attempts + 2/2 repro boots):
"PANIC: Kernel stack overflow in task 307 (fault_addr=0x0, rip=0x0)" —
ROOT CAUSE = KERNEL STACK EXHAUSTION by the recursive symlink resolver.
Decode chain (all evidence in run/repro-nftw01.log + gdb dumps):
1. The call-trace frame below trap_handler was the fault STUB's return
   address (mis-symbolized as census_peek3::is_userva / AlignedStorage —
   DWARF pollution over the stub region); the real RIP never printed.
   Added rip + kstack bounds to the panic line: rip=0x0 (instruction
   FETCH at NULL), rsp 19KB BELOW task 307's stack bottom.
2. KSO census (new, in exception.rs): task 307 cpu1 slot CORRECT, rsp0
   CORRECT, no dead-task resume, no foreign-sp switch-in, no
   freed-stack/rsp0 staleness — the freed-slot theory died; the plain
   classification was right: the task overflowed its own stack INTO the
   adjacent (free) kstack slot [0x7c5a8000,0x7c5bc000) of the 256KB
   block, tearing through stale "./loop" path-buffer data; a return
   through a zeroed slot jumped to rip=0.
3. Overflow-zone frame dump (new): follow_symlink/follow_symlink_hinted
   frames at a measured 2240-BYTE STRIDE per nested hop. The resolver is
   depth-capped at 40 (MAX_SYMLINKS, review 5.1) but 40 hops x 2240B =
   ~90KB of chain frames alone — past the 80KB x86_64 kernel stack
   before the counter can return ELOOP. nftw01's tree has the symlinks.
4. FIX: Kernel.toml kernel_stack_size_x86_64 81920 -> 131072 (128KB,
   riscv64 parity; this file's own history documents the same lesson for
   riscv64), kstack slab 3 slots/block -> 2 (zero tail waste; buddy
   fallback = one perfect order-5 block). OH-floor memory cost +48KB/
   stack (~+125MB at 2690 stacks) — flagged to coordinator for the
   merge gate (mm guarded domain).
5. Also landed (b5480343): KSO census + overflow-zone ra dump + dead-task
   resume guard at switch-in + rsp0 skip/free-vs-live-rsp0 witnesses +
   freed-stack ring. All silent on clean boots; they convert this panic
   class into named prints.
6. GATES: x3 builds rc=0 (x86_64/riscv64/aarch64); make test GREEN
   (run/maketest-128k.log "All tests passed!"); repro-nftw.sh x2 clean
   (nftw01 completes, rc=1 = its r21-class real FAIL, SCAN_DONE).
7. Long-term note for the fs lane: the reference kernel resolves symlink
   chains ITERATIVELY (link_path_walk) — 40 hops cost ~0 stack there.
   Making follow_symlink iterative would decouple stack size from path
   shape entirely (big surgery in fs/vfs.rs; NOT done this session).

## Mystery QEMU (recorded, not touched)
pid 1192385 (20:42), cwd=/home/william/rux-agents/compat-r3, boots
compat-r3's own kernel with init=/test/ltpscan + -gdb tcp::1235 — the
compat lane's QEMU, NOT this lane's.

# LTP r22 SESSION (2026-11-02, successor agent; branch ltp-r21 synced main@2fd5215f)

Mission: retest the 2 open r21 items, kernel-fix what is real, full r22 scan on
x86_64+KVM (baseline to beat: r21 = 1010/1869), results into results/, push.

## Open items — both CLOSED by 2x isolation on main@2fd5215f
1. **Blocking-socket class (recv01/recvfrom01/send01/sendto01/connect01/
   accept4_01/accept02/recvmsg01/sendmsg02)**: PASS 9/9 in BOTH isolation
   passes (run/iso22-open-{a,b}.txt, KVM, 10-test chunk). The r21
   TIMEOUT/FAIL class was REAL then and is FIXED by the virtio poll-deadline
   + kick-and-walk work now in main. Expected r22 recovery: +9 vs r21.
2. **rt_sigaction01 (signal 35 SIGILL)**: CRASH rc=132 at "signal: 35" in
   BOTH passes — deterministic. Matches the r21-bugs ground truth (same
   musl-built binary dies on the reference host kernel; musl never fills
   old->sa_restorer, the test installs garbage as SA_RESTORER). NOT a
   kernel bug; cannot pass with musl-built LTP on ANY kernel.

## NEW REGRESSION FOUND: chunk@700 boot panic (deterministic 4/4)
Full scan launched detached (run/r22-scan-driver.log, logs in
run/iso22-logs-full/). Chunks 0-600 clean (~53-90s per 100). Chunk 700
(tests 700-799, first test nftw01) panics the kernel IDENTICALLY on every
attempt (same rsp=0xffff88807c5bba50, same registers, fault_addr=0):
  PANIC: Kernel stack overflow in task 307 (fault_addr=0x0)
  Call Trace: ... do_page_fault -> handle_page_fault -> trap_handler ->
    <core::slice::sort::stable::AlignedStorage<usize, 4096>>::as_uninit_slice_mut+0xb9
Facts:
- Panic occurs right after SCAN_BEGIN + PARKWALK-ARMED + DEFERFREE prints,
  BEFORE the first RES line — early in nftw01 (first test of the chunk boot).
- rbx=0x7ff000006430, r12=0x7ff0000064f1 (USER pointers), r14=8, CR2=0:
  a kernel-mode NULL deref while a STABLE SORT of usize elements was live
  (symbol base 0x807353df confirmed via ksyms INDEX — symbolization exact).
- r21's kernel ran the SAME chunk clean (nftw01 FAIL rc=1, no panic), so a
  regression between the r21 scan base and 2fd5215f. Prime suspects by path:
  munmap chunking (bbcc5cae), MAP_FIXED teardown (4c71a4f5), fs/bio closure
  fix (619dc8cc), softirq/heal rounds — NOT yet pinned; the caller of the
  sort is unknown (unwinder garbage below the trap frame).
- Blast radius if unfixed: chunk 700 WEDGEs 100 tests (~72 r21-PASSes).
- Repro rig ready: run/repro-nftw.sh (boots run/rootfs-x86_64-probe.img =
  plain copy of the shared LTP image + zz_shmprobe/zz_shmchild injected via
  debugfs; chunk.txt=nftw01; lets the KERNPANIC dump drain fully — the scan
  driver kills QEMU 2s after "kernel panic", truncating the dump).
- Rerun rig ready: run/rerun-chunk.sh <start> <end> <tag> for post-fix
  chunk re-runs.
- TOOLING OUTAGE NOTE: the agent shell (Bash) was down for >2h mid-session
  (all spawns "cancelled before spawn"); the detached scan survived. When
  Bash returns: (1) `grep -n parkwalk kernel/src/sched/sched.rs` — the
  PARKED-LEAK parkwalk printer IS in sched/sched.rs (kernel/src/dfx/
  leakorigin.rs:701: "called from the sched.rs parkwalk printer"); it
  prints PARKWALK-ARMED at first park and is the ONLY unmatched always-on
  walker (eliminated by full-file reads: wakewatch.rs, wildwatch.rs,
  lostpark.rs, deferred_free.rs, wait.rs, elf.rs, readahead.rs, the bio
  WritePlan path, sys_mmap), (2) boot the repro for the FULL dump +
  addr2line the faulting rip, (3) fix, rebuild, rerun chunk 700 (or full
  scan if time), splice results.
- WORKING THEORY (fits the exact signature): the fault is INSIDE the sort
  scratch read (as_uninit_slice_mut+0xb9, CR2=0) — i.e. the sorted
  collection's data pointer is NULL. If the parkwalk collects victim pids
  into a Vec<usize> and sorts for deterministic printing, a corrupted Vec
  descriptor (ptr=NULL, len>0 — wild-write family, the fleet's known
  residual engine per wildwatch.rs) yields exactly this panic. The dump's
  syscall ring + KILLDUMP extras (truncated by the scan driver's 2s kill)
  will confirm; run/repro-nftw.sh lets it drain.

## execl family (7 tests, r21 class-a real) — probe built, not yet booted
execl01/execle01/execlp01/execv01/execve01/execve06/execvp01 all fail
"TBROK: Test haven't reported results!" — the exec'd child DID report
(TPASS printed, EXITGRP code=0 in iso21 logs) but the parent's MAP_SHARED
results page never sees the child's stores. LTP IPC: parent ftruncate(4096)
+ mmap SHARED on /tmp (ext4 — no /dev/shm in image), child re-opens+mmaps
after exec. Probe pair compiled (musl x86_64): run/zz_shmprobe.c + zz_shmchild.c,
three variants (double-mmap same proc / fork-inherited / fork+exec fresh-mmap),
verdicts designed to fit the ltpscan 220-byte tail. Boot via the probe image
after the panic work (one QEMU at a time).

## f1/f2/f3 (r21 EXECFAIL): NOT built by the x86_64 LTP build — the `ld`
command datafiles (testcases/commands/ld/datafiles/f1.c...) were never
compiled/installed into output-x86_64. Rig gap (3 tests), not a kernel bug.

## Known-already-fixed-on-main r21 losses (expect r22 recovery, no work):
coredump b835e56e (kill11 waitpid01), syslog d98f3272 (syslog12), sem layout
56ce13a8 (semctl01 semctl07 semget01), process_vm ea40da3c (process_vm01
readv02 readv03 writev02), sched EINVAL trio ea2df4b4 (sched_getparam03
sched_setparam04 sched_rr_get_interval03), sockets (9, above).

## SCAN RESULT (raw, unspliced)
run/results-r22-raw.txt (3431s, x86_64+KVM, main@2fd5215f):
  PASS=966 FAIL=258 TCONF=482 CRASH=30 EXECFAIL=17 TIMEOUT=14 POISON=2
  WEDGE=100  <- ALL of chunk 700 (tests 700-799) lost to the boot panic
Headline 966 vs r21's 1010 (-44) is PANIC-DRIVEN: r21 scored ~72 PASSes in
the 700-799 range, so on the comparable 1769 tests r22 is ~+28 (the
r21-bugs fixes + sockets + sched trio + ftest04 recovery visible live:
"RES|ftest04|0|Test passed" in chunk1400 log). PRIORITY 1 for the successor:
fix the chunk-700 panic, rerun tests 700-799 (bash run/rerun-chunk.sh 700
799 r22c7), splice into results/results-r22.txt (currently only the raw
copy exists), then run run/r22-pipeline.sh for the delta table.
NOTE: pipeline script run/r22-pipeline.sh copies raw -> results/, prints
class counts + delta vs r21 (adapt as needed after the splice).

## LTP IPC path fact (for the execl-family fix)
The rig's LTP uses a FIXED ipc path: guest env LTP_IPC_PATH=/tmp/ltp-ipc
(companions TBROK "File /tmp/ltp-ipc does not exist!" when run standalone —
chunk1700/1800 logs). /tmp is the ext4 root (boot mounts tmpfs only on
/dev/shm and /run). So the execl01-family coherence failure is on the EXT4
MAP_SHARED arm (kernel/src/arch/x86_64/mm/page_fault.rs "session 25"
filemap_pin_cache_page path), NOT tmpfs (tmpfs mmap is separately
documented-unsupported in kernel/src/fs/tmpfs.rs:31 — future landmine if
any test ever mmaps /dev/shm files). waitid08's tst_checkpoint ETIMEDOUT
(chunk1300 log) is the same page's futex leg.

## Successor immediate queue (in order)
1. Bash-recovery burst: grep PARKWALK in kernel/src/sched/sched.rs; boot
   run/repro-nftw.sh nftw01 (full KERNPANIC dump); addr2line the rip.
2. Fix the chunk-700 panic; x3 builds + make test; rerun chunk 700;
   splice; rerun run/r22-pipeline.sh for the real r22 delta.
3. Recompile+reinject the V1-V4 probe (run/zz_shmprobe.c has V4 now;
   the injected binary is the older V1-V3 build): musl-gcc -O2 -static,
   debugfs-write zz_shmprobe/zz_shmchild/zz_shmchild2 into
   run/rootfs-x86_64-probe.img, chunk.txt=zz_shmprobe, boot, read the
   ZZP verdict line. v1/v2/v3/v4 discriminate: double-mmap / fork-inherit
   / exec-fresh-mmap / exec-futex-wake coherence on ext4 MAP_SHARED.
4. Commit results/ + TASK.md; push origin/ltp-r21.

## Mission
1. Reconstruct deleted infra from git (main + `git branch -r | grep archived`): the LTP runner, the 1,869-case list, the per-round results layout (convention /home/william/rux-agents/ltp-r<N>/results/). test/patrol-runbook.md + test/suite.sh carry conventions. LTP source: userspace/linux-ltp/ltp-20240524.tar.xz (extract-build convention like toybox; musl SDK via `make sdk`; soft-fp overlay discipline userspace/build-softfp.sh — BUG-S005 zcb pin).
2. Build kernel (riscv64) + LTP with the pinned march.
3. Full 1,869 scan (detached/setsid; long runs survive tool timeouts). Known flake families (rerun 2x in isolation before counting new failures): accept02 getrusage04 kill02 genfmod rename/statfs.
4. Results to /home/william/rux-agents/ltp-r20/results/. Deliver r20 vs r19 delta table.
5. New real failures: triage against recent main merges (aarch64 merge b238ebd1 shared diffs: virtio device-ID table, wake floor per-arch, ELF machine checks, boot-table honesty) — compare failing syscall surface vs merge diff.
6. Small kernel fixes welcome (small commits, English, NEVER "Linux"); rerun affected family.

## Discipline
One QEMU at a time (check ps — other lanes live); -smp 2 -m 1G; kill only yours; pkill FORBIDDEN. make test stays green; x3 builds rc=0 if kernel touched. Push origin/ltp-r20.
Final message: r20 number + delta table + flake/real verdicts (2x rerun evidence) + fixes + results path.

---

# Perf review: mm

Focus: MEMORY — p3a anon fault med 0.08µs but p99 581µs (bimodal: what is the slow path paying?); P4 fork+exit 9ms / fork+exec 13ms (reference ~1ms class); TLB shootdown cost; mmap/munmap paths; copy_page_table_cow walk; vma tree operations; the allocator family (small_alloc/buddy/slab — the OH campaign named phantom preempt-count acquire sites there; the REST share, magazine discipline); demand-fill vs fault-around (aarch64 noted anon fault-around deferred; x86 batch fill exists — compare).

Perf-review lane rules (binding)
- Mission: find where Rux is SLOW vs the reference kernel and close the gap. Target: 10x-class improvements on the lagging metrics. The user mandate: every interface metric where we lag the reference must be examined.
- GROUND TRUTH: the reference kernel bzImage is at /home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images/bzImage. test/pbench_boot.py --bzimage <path> boots ANY bzImage with the pbench P1-P7 battery — run the SAME battery on the reference kernel and on your rebuilt Rux kernel to get true A/B columns. This is the primary tool: measure reference, measure ours, fix, re-measure.
- Known instrumented gaps (from syscost/OH campaign, x86_64 debug): futex wake→run tick-quantized 10-108ms (reference: µs); openat avg 252µs; p1 null syscall ~4.5µs; p2 ctxsw 15.4µs (debug); p6_append ~640µs; write_fsync ~12ms; mkdrmdir ~20ms; unlink ~1ms; pipe roundtrip ~19µs; UART print 0.36-0.93ms/line; OH boot to launcher.ready 254-541s vs reference 14.4s (~26x whole-system).
- Every fix ships with NUMBERS: pbench median before/after (same boot config), or syscost per-op average before/after. No numbers = not merged.
- Correctness gate: `make test` green (riscv 1198) + x3 builds rc=0. If you touch aarch64 paths, `make test PLATFORM=aarch64` green too.
- Small commits, English messages, NEVER the word "Linux" in commits/docs — write "the reference kernel".
- Record findings/fixes/numbers in run/perf-findings.md; push origin/perf-<scope>.
- One QEMU at a time (check ps; many lanes run); kill only your own; pkill FORBIDDEN.
- Worktree: THIS one. Never write to /home/william/workspace/Rux.

---

### Landed (all pushed)
1. merge main e8cc17ec (rotation-proof heal, 2s bound, cumulative bar).
2. **WAKE-HEAL** (sched.rs, wake_up_enqueue_opt tail): a task about to be
   woken, GRQ held, owned by no CPU slot (verified), not mid-switch-out
   (with 50-jiffy stale-marker confirmation — a stale SWITCHING_OUT entry
   was the silent forever-skip for the carrier tasks), not PF_EXITING,
   carries NO legal guard by construction (sc265's conviction) → its
   phantom preempt_count is zeroed AT WAKE so the monopoly never forms.
   Skip-reason census: `why=e/s/c` budgeted 60; heal report budgeted 300.
   lb18 measured: fires 268/boot, strands 13 (vs 21-36 before), lb20
   early: 0 strands/34s. ALL observed skips are why=c (task in its own
   slot — legit pre-sleep window OR lying busy slot; the lying-slot
   adjudication needs the frozen-exec-clock check, NOT done).
3. **tick-heal now sets need_resched** after clearing base bits (the zero
   alone left the incumbent running to user-return — sc267).
4. **kpreempt=1 cmdline arm of the IRQ-exit kernel-preempt hook**
   (trap.rs, default OFF, syscost-style cmdline verdict cache).

### VERDICTS (evidence in run/oh-lb19b-serial.log)
- **kpreempt is NEGATIVE in the current tree**: DEADLOCK spinlock-stuck
  prints on BOTH cpus from 54s, strands EXPLODED to 40. Mechanism: the
  2s tick-heal false-heals REAL guards held >2s (holders are themselves
  collapse victims) → preemptible() lies → the hook preempts a lock
  holder mid-critical-section → the DEADLOCK lock 0xffff88807d2876d0 is
  the same slab-region instance the underflow drops name every boot.
  Full kernel preemption needs the false-heal problem solved first.
- **The leak is a steady-state ENGINE, not one-off**: lb17 tail shows the
  same pids (0x13e/0x219/0x24c/0x425 = OS_IPC worker threads, e.g. pid
  318 = first OS_IPC worker after key_enable) re-healed every ~200ms
  with was=0x10001, underflow at spinlock.rs:461/rwlock.rs:457 drops
  (lock instances 0xffff88807d28a2d0/0xffff88807df62290 heap/slab
  region — per-inode/per-socket Spinlock family per sc73), re-leak
  immediately. Producer remains the double-run/wild-write family.
- The lock at 0xffff88807d2876d0: slab-region heap object embedding a
  Spinlock (inode.state / socket state family), deterministic address
  across boots, contended by both CPUs for seconds at a time.
- SLOW-TXN exec times up to 41.7s (device_manager code 14 during the
  install stream) — exec-time class, not queue-time: the SERVER thread
  itself runs 41s (crypto+syscalls under the collapse), same family as
  the deviceauth 129s / asset_service 135s census stalls.

### lb20 ENDGAME (read before next boot) — the wedge is now NAMED
lb20 (full 2400s, WAKE-HEAL + need_resched insurance): the healthiest boot
yet, AND the account-chain wedge is now a concrete starvation cascade:
1. accountmgr SA LOADED for the first time ever ("SAFWK: StartSystemAbilityTask
   accountmgr SA:200 init finished, 252 ms" @221.9s, pid 976; SACB callbacks
   DELIVERED to tid 976 x16 @268-311s — the samgr->client direction works).
2. softbus_server (pid 724) registered @228.8s then went SILENT — its worker
   never answers. Its pick claim was reset once (STALE-CLAIM-RESET by_cpu=0
   pid=724 @841.7s) with no recovery. This is the R85 "linked but starved /
   pick-skip wedge" family (orphaned on_cpu mark / stale claim), NOT a leak.
3. Consequence cascade, live in the log: device_manager (pid 568) code-3
   handler waits ~5.24s (softbus LNN timeout) PER CALL, x102 SLOW-TXNs from
   distributeddata's DB_TaskExecutor in a 5.75s retry loop; 104 stale-binder-
   buffer reclaims of pid 568 (its own waiters' reply buffers age out);
   distributed account queries (SAREG code=4 ohos.account.f...) spam accountmgr
   x dozens/second from 355s on; bootevent.account.ready never SET (the
   param_watcher wait at 211s hangs); useriam watchdog-killed at 641s (own
   OS_DfxWatchdog) and restarts (687->1064->1135); screen stays md5 06b933b4.
4. NO PARKED-LEAK on any softbus thread; last WAKE-STRANDED at 315s (pid 318
   behind incumbents pid 684 pc=0x102 and pid 1 pc=1) — the strand/heal
   machinery QUIETED after 315s. The final state is a stable starvation
   equilibrium, not a collapse in progress.
### Session close (third successor, 2026-10-31 ~17:50) — LADDER POSITION
Ladder 1 (foundation survives) NOT yet held; the wall moved from "generic
collapse" to ONE concrete cluster. Facts at close:
- lb21 (dfx=wwcensus, same kernel): strand volume COLLAPSED to 6/boot
  (strand_max 10.4s; lb15/16 class was 17-36) — the heal stack (rotation-
  proof 2s tick heal + WAKE-HEAL + need_resched insurance) works. All my
  QEMUs are dead; screen md5 06b933b4 (frozen logo) on lb20/lb21.
- accountmgr SA loads (both boots); samgr->client callbacks work; the
  remaining wall is the softbus/device_manager/accountmgr distributed-
  cluster handshake: every call in that cluster runs multi-second (5.25s
  device_manager timeouts on lb20; softbus->accountmgr code-14 SLOW-TXNs
  on lb21), so activation never lands inside the watchdog windows.
- Stale-claim resets keep regenerating (lb20: pids 497x2/457; lb21: 532x2/
  584/1092) — the phantom-mark producer is alive but bounded.
- Branch oh-launcher pushed through 7c850d6d + TASK commits; x3 builds
  rc=0 (x86_64/riscv64/aarch64); make test ALL PASSED twice (lb18/lb21
  maketest logs) on the final tree.

### FOURTH-SUCCESSOR ENTRY POINTS (in order)
1. The softbus starvation (lb20 evidence above): first boot with
   `dfx=wwcensus` (lb21's recipe), watch pid 7xx softbus_server threads'
   W3-RUNWAKE-REFUSED / W3-FROZEN-EXEC-SLOT lines + the PICK-SKIP reason
   codes in fair.rs's pick decision legs. If a softbus worker is linked-
   but-skipped with why=on_cpu, the R85 stale_oncpu heal's relink leg is
   the fix site (it fired STALE-CLAIM-RESET on 724 at 841s on lb20 without
   recovering the task).
2. The re-leak engine (unchanged): OS_IPC worker carriers re-acquire
   phantom +1 every ~200ms (lb17 tail); drops underflow at spinlock.rs:461/
   rwlock.rs:457 with heap lock instances in the slab region
   (0xffff88807d28a2d0/0xffff88807df62290/0xffff88807d2876d0 — the last
   one DEADLOCKED both cpus under kpreempt). Root remains the double-run/
   wild-write family (sc265/266/73 convictions; r83hunt is the tool).
3. kpreempt=1 is armed-but-NEGATIVE until guards-held->2s disappear
   (the false-heal preempts real lock holders; lb19b DEADLOCK proof).
   Prerequisite: cond_resched at lock-free boundaries of the long kernel
   loops (fork copy / exit unmap / rmap batches) OR the producer fix.
4. timerfd_create storm caller still unnamed (4k/s; the 10-line TFDC
   budget only caught boot-start callers: bluetooth/audio).
=> OH-side note: even with accountmgr up, the DISTRIBUTED account path
   (ohos.account.foreign/distributed) drags device_manager/softbus into the
   critical path at boot. The hmdfs-disable image edit killed hmdfs, but
   distributeddata's device_manager dependency remains. If the kernel
   starvation is fixed, this chain should complete on its own.

### Boot inventory this session
lb16 (merged heal): anim.ready 221s, distributeddata watchdog-killed
  (OS_DfxIpcCheck KOT-KILL 15 members) at 260s, wedge same class.
lb17 (+WAKE-HEAL v1 silent): anim.ready 254s; console stalled ~300s;
  steady re-leak engine first captured. NOTE: lb17's "launcher.ready
  marker" was a FALSE match on "launcher.ready not set" — lb-boot.py now
  rejects "not set" lines.
lb18 (+skip census +stale-marker confirm): anim.ready 226s, heals 268,
  strands 13, accountmgr started 303s, killed at 650s mid-account-phase.
lb19/lb19b (kpreempt=1): the DEADLOCK verdict above (killed early).
lb20 (running at write time): full 2400s, current branch.

### Successor entry points (in order)
1. lb20's endgame: did the account phase complete / foundation survive /
   launcher spawn? (watch AccountMgrService / ActivateDefaultOsAccount /
   StartUser / AMSI1640 userId / appspawn com.ohos.launcher).
2. The remaining strand class is why=c lying busy slots: add the
   frozen-exec-clock adjudication (W3-FROZEN-EXEC-SLOT machinery) to the
   WAKE-HEAL's curr_on skip — a slot naming a task whose sum_exec_runtime
   is frozen for 200ms+ while its state is SLEEPING is lying.
3. The per-op wall (26x): syscall wall-times include wait-behind-
   incumbents; after the monopoly fixes the residue is COW/fault cost
   (rmap 28% of kernel ticks) + the timerfd_create storm (4k/s caller
   STILL unnamed — bump the TFDC budget behind a dfx switch and boot once).
4. The false-heal vs kpreempt dependency: real guards held >2s exist;
   bounding them (cond_resched at lock-free boundaries of fork copy /
   exit unmap / rmap batch loops) is the prerequisite for arming kpreempt.
  (1096/1096); branch pushed through fa31bfb3.
- Successor entry points, in order: (1) the stranded-wake/preempt-count hunt
  (WAKE-STRANDED 1-2.6s at pc=1-2 + PARKED-LEAK pc=1 prints — leak suspects,
  needs a preempt-count origin shadow like the leakorigin hook); (2) the
  userId-0 launcher hole (see above) — image-side or perf-side; (3) once a
  boot keeps foundation alive through activation, the launcher should spawn
  (systemui already does) and the GL/scanout chain is proven.

## FOURTH-SUCCESSOR SESSION (2026-10-31 17:00-18:xx, branch oh-launcher @ 49df47b8+)
Base c23045fe + this session. THE WALL WAS MOSTLY A MISSING /data.

### THE BIG FINDING — every boot from lb16 through lb26 ran WITHOUT /data
1. a6b01fd9 (main, cr2 csum review, 11:49) tightened superblock_csum_ok
   from a 16-bit to the FULL 32-bit compare (correct per e2fsprogs).
2. run/userdata-fresh.img (cooked 08:26 by the PRE-fix kernel whose sb
   sealer wrote only the low u16, snapshot=on ever since so it looked
   pristine) fails the new check: stored le32 0x4e8183c6 vs computed
   0x5b8283c6 — low 16 bits match (why lb1-lb15 mounted it), high 16
   are the stale old-seal. Verified on-host with a crc32c recompute.
3. Effect: EVERY boot lb16..lb26 (the whole third-successor campaign,
   INCLUDING lb20/lb21) logged `mount[10] vdf -> /data: -5` x3 +
   "ext4: superblock checksum invalid". /data never mounted; the
   distributed cluster (softbus LNN store, distributeddata RDBs,
   accountmgr osaccount DB) ran on an unmounted read-only empty /data.
   The "softbus starvation wall" was largely SOFTBUS HAVING NO DATABASE
   DIRECTORY — its LNN init can never finish, so every device_manager
   LNN query burned its 5.24s timeout (x102 on lb20).
4. FIX (this session): run/userdata-fresh2.img = fresh mke2fs 16G ext4
   (T3 recipe; sb csum passes the full-32-bit check bit-for-bit) +
   USERDATA env in lb-boot.py. NEVER boot the old userdata-fresh.img
   again on a post-a6b01fd9 kernel — recook instead.

### lb27 PROOF (release kernel + fresh /data; the healthiest boot ever)
- /data mounts rc=0 first try; no dsoftbus mkdirfails.
- SetBundleQuota for /data/app/el2/100/base/com.ohos.launcher @138s;
  StorageDaemon StartUser(100) COMPLETES in 64ms @237s;
  startup.appspawn.lockstatus_100 SET; USER_ADDED + USER_UNLOCKED fire;
  bootevent.bms.main.bundles.ready @237.9s; wms.fullscreen.ready TRUE;
  ZERO device_manager code-3 timeout chains (lb20: 102); 8521220 GONE
  (the userId-0 hole closed itself once /data existed); SLOW-TXN total
  18/boot. The account ACTIVATION completes.
- NOT yet landed: the AMS launcher START (no AMSI1640, no
  com.ohos.launcher process, screen still the frozen logo md5
  06b933b4 through 911s). bootevent.account.ready is watched (pid 1
  msgId 26) but never SET — find what sets it on the reference kernel.
- lb28 (dfx=ams46): StartUser oneway LATCHED (node=12710 pid=876
  foundation @283s) — this boot's SA-load phase ran the slow samgr
  code-7 family again (29-75s exec), unlock not reached by 663s.
  Boot-to-boot variance in the huks/account keygen path decides it.

### Scheduler work landed (commits 49df47b8 + 0fa06de4 + bb82cac2)
1. CLAIM-SWEEP (fair.rs claim_sweep + scheduler_tick hook): the pick-side
   stale-claim heal was ORDER-DEPENDENT (walks stop at the first allowed
   leftmost; a mid-tree victim with frozen vruntime is never examined —
   lb20's pid 724 lost 579s between resets). Once per 100 jiffies one
   CPU sweeps the whole timeline under the GRQ with the pick-side
   adjudication: {on_cpu=false, claim!=-1} owned by no slot/switch-out
   -> reset claim; linked+on_cpu orphaned-mark shape (ownerless +
   50-jiffy exec-frozen orphan-watch confirm) -> full-qword release.
   Census line (dfx=wwcensus) names pids of both classes. lb22-lb25
   validated: zero false heals (every oncpu sighting was owner-backed);
   no cluster thread was ever stale-claimed on those boots — the claim
   stranding class was NOT the softbus killer (the missing /data was).
2. tfdcensus (dfx=tfdcensus): windowed TFDC tap (first 16 calls per 5s
   window). Storm callers named: companion_device (42), useriam (20),
   time_service (10), OS_IPC workers, usb_host. Entry point 4 CLOSED
   (naming); the storm is fd-churn COST, not a timerfd leak — timeouts
   expire on schedule.
3. TXN-HANG lens (lb24, dfx=txnhunt+poolcensus): binder delivery is
   CLEAN (zero queued todo across the whole boot; no stuck inbound
   sync txn into softbus). The 30-55s waits are userspace-INTERNAL
   futex waits inside executing handlers (accountmgr tid 976 sys=202
   FUTEX_WAIT 31s; foundation main thread 55s; nested BWR reads 50s) —
   exec-class compute/condition waits, NOT kernel scheduling strands.
4. leakhunt (lb25, dfx=leakhunt): the phantom base-bit (+1) acquire
   sites on the carriers (pids 318/512/537/601): spinlock.rs:292,
   mm/slab.rs:628, mm/small_alloc.rs:283, mm/buddy_allocator.rs:857,
   sync/rwlock.rs — the ALLOCATOR-lock family. Producer still at large
   (r83hunt territory; the wild-write/replayed-acquire split unknown).
5. Release kernel: target/x86_64-unknown-none/release/bzImage
   (build-release target) boots the image fine — 6x wake throughput,
   appfwk.ready 132-169s (debug: 217-328s). Same wall without /data
   (lb26), clean with it (lb27) — proves the wall was not compute.
   NOTE: release strand tail exists (stranded=70, strand_max=24s class
   on lb27) — worth one syscost look before relying on release boots.

### FIFTH-SUCCESSOR ENTRY POINTS (in order)
1. THE LAUNCHER START: boot release kernel + USERDATA=run/userdata-
   fresh2.img + dfx=ams46,txnhunt; watch AMSI1640/SwitchToUser/appspawn
   spawn of com.ohos.launcher after lockstatus_100. Variance: lb27-class
   boots unlock at ~237s; lb28-class get stuck in the samgr code-7 30s
   exec family — if an ams46 boot shows the StartUser oneway delivered
   but not executed (the walled-draw shape the arsenal was built for),
   that print names the drop site.
2. bootevent.account.ready is never SET (only watched): compare with the
   reference kernel (14.4s boot) which service/param job sets it; likely
   an init condition job that depends on a service which is slow/dead.
3. The re-leak producer (unchanged): allocator-lock acquire sites above;
   r83hunt + leakhunt together on one boot.
4. Perf-gate note for the coordinator: this session's sched changes
   (CLAIM-SWEEP = once-per-second GRQ-held timeline walk) are Tier-2
   guarded-domain (sched/); needs syscost + pbench gate before any
   merge to main.

### Boot inventory this session
lb22 (dfx=wwcensus,schedtrace, debug): claim sweep live, 17 pick-side
  resets, cluster threads never stale-claimed; timeout loop present.
lb23 (+full sweep): softbus pid 731 REPLIES code-8 in 2.6s (not silent);
  TFDC windowed census first data; timeout loop still present (no /data).
lb24 (poolcensus+txnhunt): TXN-HANG futex-wait picture; no queued todo.
lb25 (leakhunt+futextrace): phantom acquire sites named; no key-mismatch
  prints (no lost futex wakes observed).
lb26 (RELEASE kernel, no /data): same wall → wall is not compute.
lb27 (release + fresh /data): MILESTONE boot — see above.
lb28 (release + fresh /data + ams46): slow SA-load variant, evidence in
  run/oh-lb28-serial.log.

### lb29 addendum (clean-config retry, release + fresh /data + wwcensus)
Slow variant: unlock at 746s (lockstatus_100 set), USER_UNLOCKED events
fire 806-821s, NO accountmgr reset this time — and STILL no launcher:
AMSI1640 never prints, and com.ohos.systemui NEVER SPAWNS either (0
mentions; lb10-era boots DID spawn systemui from StartUser). So the
post-unlock gate is upstream of both ability launches: foundation
receives the StartUser(100) oneway (lb28's AMS46 latch @283s) but the
SwitchToUser/user-start ability chain never executes inside foundation
on these boots — the ams46 walled-draw family. Fifth successor: get an
ams46 boot through a 237s-class unlock (lb27 config luck) and read the
AMS46 read/free/retire + census lines for node 12xxx — they name
whether the oneway is parked in the async slot, read and dropped, or
racing a dead reader.

## FIFTH-SUCCESSOR SESSION (2026-10-31 20:00-23:xx, branch oh-launcher @ a7189470+)
Kernel delivery is CLEAN; the wall is INSIDE foundation userspace, and it
has TWO shapes. All boots: release kernel, LOGLEVEL=6, SMP=2,
USERDATA=run/userdata-fresh2.img, dfx=ams46(+txnhunt/periodic).

### The ams46 verdict (lb30/lb32/lb34/lb35/lb36/lb37)
1. **lb30**: ENQ 807.707 (accountmgr→foundation AMS node, oneway 0x11) ->
   READ by foundation MAIN thread 1ms later -> BC_FREE 8ms after. The 30s
   silence-rule drop NEVER fired — kernel delivery is NOT the wall. The
   old "ams46 walled-draw" family is closed: the oneway executes.
2. Instrumentation landed for the follow-window (all pushed):
   - RIPPLE (a7189470): after the traced READ, print every txn whose
     sender/receiver is foundation or the StartUser sender, 120s, budget 640.
   - THREAD-SNAPSHOT (19b195f3): the reader thread's in-flight syscall
     (kernel-stack entry frame) + 16-entry nr-ring, every 10s.
   - HILOG-AMS cohort at WARN in the unix-socket hilog tap (unix.rs): BMS/
     AMS/accountmgr verdict lines on the console at loglevel 6.
   - AMS46-R parcel hex (80B) + ENQ sender euid.

### Shape A — the LOOP (lb34, lb36): DECODED, root cause named
- Chain on the console: `CreateNewUser user(100) start` → `startUser in
  service:100` → `foregroundUserId:0` → `SwitchToUser old=0 new=100` →
  `waiting query highest priority ability info completed` x588+ forever.
- THREAD-SNAPSHOT: reader tid in nanosleep (sys=35) + clock_gettime churn
  = AMS's StartHighestPriorityAbility isBoot=true retry ladder
  (ability_manager_service.cpp:7905): `ImplicitQueryInfoByPriority` keeps
  returning false → usleep → retry, NO attempt limit on the boot path.
- The query needs com.ohos.launcher's PER-USER record for user 100, which
  comes from CreateNewUser(100)'s per-user install stream
  (OnCreateNewUser → InstallByBundleName for EVERY preinstalled bundle →
  installd SA 511). On our kernel that stream's installd loads burn
  15-101s EACH ("Start dynamic proc:installs,511,105_*ms" — samgr's
  ServiceControlWithExtra against init timing out) and repeat.
- FIX LANDED (image): run/lb-mkimage2.sh → run/system-lb2.img flips
  installs.cfg "ondemand" true→false. Result on lb36/37: 0 dynamic
  loads, CreateNewUser(100) COMPLETES (start+end lines). Necessary.
  NOTE: sa_main for an on-demand-profile SA still only registers its
  PROFILE at boot (samgr code 10 = ADD_ONDEMAND); the live SA loads on
  the first LoadSystemAbility(511) — now into a resident process.

### Shape B — the FAST-FAIL (lb35, lb37): errcode pending lb38
- `CreateNewUser end` (success!) → `startUser in service:100` →
  OnStartUserDone oneway (code 0x1, dlen 68) back to accountmgr within
  ~5ms → accountmgr logs "Failed to AbilityManagerService in call back"
  → activation fails, rollback, NOTHING spawns. No foregroundUserId/
  SwitchToUser lines → the fail is INSIDE CheckUserParam (AMS
  user_controller CheckUserParam), and NO IsOsAccountExists binder call
  nor samgr lookup preceded the callback (ripple). Kernel euid stamp at
  ENQ = 3058 (correct, printed). The errcode rides at parcel offset 62
  (int32): 2097177=CHECK_PERMISSION_FAILED(uid leg), 2097193=
  INVALID_USERID_VALUE(IsExistOsAccount false). lb38 (80B parcel lens)
  reads it.

### CLOSE-OUT (00:30): Shape B named to its source line; errcode decoded
- lb38 parcel decode: OnStartUserDone(100, 2097193=INVALID_USERID_VALUE).
- lb40 keys landed: "failed to get os account service" (os_account.cpp:894
  GetOsAccountProxy) fires x137 in foundation 9.9-139.8s — ALL before
  samgr's "insert SA:200" (141.3s): the ohos-account kits' proxy is
  unobtainable until SA 200 registers; every osaccount query in that
  window fails with NO kernel traffic (proxy null).
- lb42 (the clean kill): CreateNewUser end -> startUser at 225.485 ->
  **UN: [user_controller175]userId not exist** at 225.486 -> callback
  error. At 225s SA 200 was long registered and NO proxy-failure line
  fired — the IsOsAccountExists QUERY RAN and accountmgr answered FALSE
  for the user it had just created+unlocked: the osaccount DB read
  (OsAccountControlManager -> distributeddata KV, the DB_TaskExecutor
  SLOW-TXN family) is the boot-to-boot coin flip. Kernel-side that is
  the distributeddata/exec-class perf wall (userspace-internal futex
  waits inside executing handlers — prior TXN-HANG decode), NOT binder.
- lb40 (the loop shape with everything right): CheckUserParam passed,
  SwitchToUser entered, ladder still spinning post-CreateNewUser-end —
  the BMS "can't find target AbilityInfo" verdict key landed too late
  for that boot; UNVERIFIED whether the ladder eventually exits.

### Successor entry points (in order)
1. Boot run/system-lb2.img + dfx=ams46,txnhunt (LOGLEVEL=6, SMP=2,
   USERDATA=run/userdata-fresh2.img). Hope for the lb40 shape and read
   the "can't find target AbilityInfo" verdict: if BMS's
   ImplicitQueryAbilityInfos misses the launcher for user 100 even after
   CreateNewUser end, check GetAllPreInstallBundleInfos' disallowList
   path and the per-user record for com.ohos.launcher.
2. Shape B mitigation candidates (image-side): none clean; kernel-side
   it is the distributeddata KV read reliability under load — the
   guarded-domain perf work.
3. The re-leak producer / samgr service-start timeouts (15-101s) remain
   the deep wall behind everything; the perf lane owns them.

### lb44-lb46 addenda (the fast-fail's engine, still one facet short)
- lb44: errcode 2097193 again; euid=3058 correct; NO IsOsAccountExists
  query ENQ, NO proxy-failure line at the moment, accountmgr did NOT
  restart — the failing call never left foundation's userspace. The
  ipc-skeleton keys ("SendRequest", "work thread") landed after lb44.
- lb45 (dfx=poolcensus): ZERO POOL census lines all boot = no proc ever
  had queued todo at census time — the "queued work undrained" pool
  shape is absent; the exhaustion shape (all workers parked as CLIENTS,
  SendRequest failing locally) is invisible to that census gate.
- lb46: reader freed the code-46 after a 160s SILENT handler (sys=16
  parked in a nested binder ioctl the whole window; no TAG_LOGI even
  printed), then OnStartUserDone(100, 2097193) again. The osaccount
  query chain under load is the engine; its exact failing leg (ipc
  work-thread exhaustion vs a 160s-stuck nested call returning false)
  is the ONE remaining unnamed hop. "SendRequest: Invalid samgr stub
  object" x2 and SendRequestInner cost:1583ms lines appear on these
  boots.
- Screen on every boot this session: the frozen bootanimation frame
  (md5 5f8b704d / 06b933b4 class).

### Artifacts this session (all committed+pushed)
- Instrumentation: AMS46 RIPPLE (a7189470), THREAD-SNAPSHOT (19b195f3),
  HILOG-AMS WARN cohort + account-kits/query-verdict keys, AMS46-R parcel
  hex 80B + ENQ euid, AMS46-R-FAIL (send failures in-window).
- Image: run/lb-mkimage2.sh -> run/system-lb2.img (installs resident).
- Boots lb30-lb42; make test ALL PASSED on the final tree; x3 builds
  rc=0 (x86_64/riscv64/aarch64).
## SIXTH-SUCCESSOR DECODE (2026-11-01 01:3x, branch @ 3ad7c3e7+)
The "160s silent handler" was an INSTRUMENTATION ARTIFACT, not a shape.
Raw-log re-derivation of all three boots:
1. **2097193 = AAFWK INVALID_USERID_VALUE, source-confirmed**:
   ErrCodeOffset(SUBSYS_AAFWK=1) = 1<<21; ability_manager_errors.h
   offset+41 (0x29) with the source comment "Result(2097193) for
   invalid userid". The ONLY return sites are user_controller.cpp:157/
   166/176 (CheckUserParam's three legs).
2. **lb44 @266.819-820 and lb45 @208.830-831 are decisive**: the code-46
   handler prints "startUser in service:100" and "userId not exist"
   (user_controller.cpp:175) 1 ms apart. The ripple window (prints EVERY
   foundation txn) shows ZERO sends between them; R-FAIL never fired;
   AMS46 lifecycle READ->FREE = 4-6 ms. So CheckUserParam's
   IsExistOsAccount leg completed in ~1 ms with NO kernel binder
   traffic and NO "failed to get os account service" log (lb44's
   HILOG-AMS budget was only at [100] then — alive).
3. **The engine, named**: `OsAccount::IsOsAccountExists` pre-sets
   isExist=false; on a cached proxy_ it calls
   `proxy->IsOsAccountExists()`. `IPCObjectProxy::SendRequestInner`'s
   FIRST line: `if (IsObjectDead()) return ERR_DEAD_OBJECT;` — silent
   (ZLOGD only), instant, no kernel contact. And `GetOsAccountProxy`
   returns proxy_ whenever non-null — a DEATH-MARKED proxy is returned
   FOREVER (the death recipient only clears on an actual death
   notification; a failed send's SetObjectDied(true) is never undone).
   So ONE earlier send to the osaccount handle that returned
   ERR_DEAD_OBJECT (kernel BR_DEAD_REPLY; the mark is also set on any
   send whose status maps DEAD) poisons EVERY later osaccount query in
   foundation for the whole boot — CheckUserParam always fails, StartUser
   always returns 2097193, the launcher NEVER spawns.
4. **Why the window looked "silent for 160s"**: the AMS_KEYS "SendRequest"
   matched SA_CLIENT's "SubscribeSystemAbility SendRequest succeed!"
   flood — budget [599] exhausted at 125.6s on lb46, dark from 125s to
   the end; lb44's AMS46-T sys=16 snapshots are all AFTER the fail
   (foundation's main looper parked in BWR — its normal idle posture).
5. **The open kernel question (lb47 hunts this)**: WHICH send failed and
   WHY — our kernel returned BR_DEAD_REPLY/BR_FAILED_REPLY to some
   foundation->accountmgr(-osaccount) handle earlier in the boot. All
   reason prints were pr_info (invisible at LOGLEVEL=6) or
   launcher-family-gated (app_txn_fail_print). Candidates: ref lost
   from proc.refs (invalid-handle), node wrongly marked dead
   (child-node-dead), owner seen dead. New taps (3ad7c3e7):
   AMS46-SENDFAIL always-on WARN (pid tid code handle BR reason,
   budget 400, reason shadow 1-18) + de-flooded AMS keys
   ('failed, handle:' = IPCObjectProxy's error verdict, 'invoker is
   null', 'Invalid samgr stub', 'SendRequestInner cost').
6. OH-source reference points: SendRequestInner early-dead return at
   foundation/communication/ipc/ipc/native/src/core/framework/source/
   ipc_object_proxy.cpp:218-247; the poison-cache at
   base/account/os_account/frameworks/osaccount/core/src/os_account.cpp
   GetOsAccountProxy (proxy_ != nullptr fast path) + IsOsAccountExists
   (isExist=false preset); AMS handler at
   foundation/ability/ability_runtime/services/abilitymgr/src/
   ability_manager_service.cpp:9596 (StartUser -> CheckUserParam ->
   OnStartUserDone on ANY failure).

## PREEMPT-HUNT SESSION (2026-11-01 02:00-07:00, branch preempt-hunt @ 1607f62e+)
The phantom preempt-count producer is NAMED: two faces, one dominant.

### Face A (DOMINANT, killed): the preempt-disabled SPIN-WAIT convoy
1. Instrument chain (commits 88dd4b82→bd4410ff): outermost 0->1 acquire
   shadow + exact caller-site attribution (#[track_caller] threaded
   through Spinlock/RwSpinlock acquire paths via preempt_count_add_at)
   + live ACQUIRE-DIVERGENT report + multiplicative slot hash (the old
   XOR mixes aliased task structs 0x1000000/0x2080000 apart — ph2's 40
   "splits" were hash collisions; ph3 with the Fibonacci hash: 1).
2. ph2 evidence (system-lb.img + userdata-fresh2.img, dfx=leakhunt,
   wwcensus, debug kernel, 1200s): 320 tick-heals, 109 underflows, 321
   strands. The >2s "phantom" incumbents name SHORT-section locks
   (dentry children/vfsmount, unix dgram_peer/recv_queue, inode icache,
   mm vma_files) — because preempt_disable runs BEFORE the TTAS spin,
   a waiter's whole 0->1 booking covers the SPIN. Behind any holder
   running a multi-second section (unix stream drain memcpy under
   recv_queue, build_path under an fs_struct read guard, mm unmap under
   the pte lock — the rmap_alt_remove rip), every waiter monopolized
   its own CPU preempt-off for the whole hold: the pc=1 WAKE-STRANDED
   incumbents, the 2s-heal stripping LIVE spin bookings, the post-heal
   SpinlockGuard::drop underflows (31+/boot), the kpreempt deadlock.
3. FIX (5f0537a8): Spinlock::lock and RwSpinlock::read/write now wait
   PREEMPTIBLY at the outermost depth in task context (TTAS with
   preempt on, disable only around the acquiring CAS, schedule() to the
   holder after a 20k budget); nested/IRQ callers keep the legacy
   preempt-off spin (mid-nesting schedule = the LTP r2 WEDGE family).
   RwSpinlock::read waits on has_writer() only. Also fixes the if-let
   temporary-extension guard in FsStruct::get_cwd/get_root (the #1
   origin family, 46 lines/boot). Holder side: unix stream drain now
   copies OUTSIDE recv_queue (1607f62e; the memcpy was a named >2s
   origin and user-page faults nested the fault path under the lock).
4. VERIFICATION (ph3 vs ph2, full 1200s boots): tick-heals 320->171
   (-47%), strands 321->221 (-31%), underflows 109->96, panics 6->2,
   SLOW-TXN median exec 5780ms->2250ms (count 32->57: fewer forever-
   wedged txns means more complete past the threshold). make test
   riscv64 GREEN (1198/0); x3 builds rc=0 (x86_64/riscv64/aarch64).
   ph4 (drain split + live divergence) REGRESSED vs ph3 (heals 281,
   WAKE-HEAL 26, underflow 136, SLOW-TXN med 7970ms) AND carried an
   exact-fit semantic divergence (it popped one segment beyond the
   buffer, draining its fds without delivering its data) — REVERTED in
   bf53ba41. The holder-side drain bounding needs a per-segment
   pop/drop/relock shape (no heap Vec, no over-pop), not the batch
   shape; future work. ph3's numbers are the branch's verification.

### Face B (residual, ~4 user-return phantoms/boot): the lying sched slot
- The definitive phantom: LEAK-ORIGIN user-return pc=1 (a +1 at user
  return can never be a live guard). Exact osites: dentry.rs:370/395
  (vfsmount/mountpoint locks), always omis=1 — the acquire ran while
  tp named the victim but the CPU's sched slot named ANOTHER task
  (oslot recorded; live ACQUIRE-DIVERGENT lines show slotpid).
- ph4 live data: divergent acquires spread across MANY short-section
  sites (dentry/unix/syscall) — the producer is the SLOT lying (naming
  a task that is not executing), not any one lock. Windows that leave
  slot != tp: pick-publish (sched.rs:2515) -> tp-swap tail on prev;
  the R56c lying-idle-slot heal; abort paths repair via
  republish_current_after_abort (verified present). NEXT HUNT: freeze
  on a live divergent acquire with slotpid, walk WHICH machinery wrote
  that slot last (slot_tag_write tags exist: SLOT_W_SCHED_PICK,
  SLOT_W_R56C — add a tag read to the divergence report).
- WAKE-HEAL/CLAIM-SWEEP/2s-tick-heal remain symptom treatment for this
  face; they are what converts a lying-slot window into a stripped
  guard + underflow.

### kpreempt=1 verdict (stretch NOT taken)
Still blocked: 171 tick-heals/boot remain (mm unmap loops under the pte
lock are the biggest residue — the rmap_alt_remove incumbent family).
Every heal strips a possibly-live guard; with the IRQ-exit hook armed
that means preempting a holder mid-critical-section = the lb19b
DEADLOCK shape. Prerequisite unchanged: bound the unmap-loop holds
(drop/reacquire the pte lock every N pages) or make the heal holder-
aware. The convoy fix removed the WAITER-side blockers only.

### Boots this session
ph1 (tap v1, last-wins sites, 374 user-return lines all pid 568 — the
  rate gate + exact sites landed after), ph2 (exact sites: the site
  distribution table), ph3 (convoy fix: the -47%/-31% numbers),
  ph4 (drain split + ACQUIRE-DIVERGENT live).

### FINAL TREE VERDICT (14d04456, pushed)
ph6 (record-only divergence, 900s, same recipe): the best boot of the
session — at the matched <=600s window vs ph2/ph3: PREEMPT-HEAL
187->114->38, UNDERFLOW 53->57->25, WAKE-STRANDED 233->138->31,
DEADLOCK 0, SLOW-TXN med 1970ms (n=15, full window), appspawn at 5.2s,
serial live to 900s. ph5/ph5b (the two boots carrying the live
ACQUIRE-DIVERGENT print) BOTH wedged on net::unix::UNIX_TABLE
(both-CPU DEADLOCK, stranded-0x10000 carriers) — print disabled
(record-only) in 14d04456; correlation-by-elimination, mechanism
unproven. make test on the final tree GREEN (ALL TESTS PASSED); x3
builds rc=0. The ph5/ph5b UNIX_TABLE wedge shape matches the pre-existing
lb19b/s14 stranded-0x10000 family — the next hunt's Face C candidate.

## EIGHTH-SUCCESSOR SESSION (2026-11-01, branch oh-ladder @ 53f4fd47)
Mission: re-measure SLOW-TXN/watchdog with the preempt-hunt spinlock fix on
main, chase the launcher ladder. Rig findings first (both boots main @ 4a757d6e,
system-lb2.img + userdata-fresh2.img + dfx=ams46,txnhunt LOGLEVEL=6 SMP=2):

### WEDGE CHECK (coordinator question answered): current main still wedges, BOTH profiles
- lb55 (DEBUG main): SA inserts stop at 16 (24.1s); first strand 12.4s at 38.1s;
  59 DEADLOCK prints from 83.7s, ALL on net::unix::UNIX_TABLE (static lock,
  nm-verified). WAKE-STRANDED up to 133s (pid 523) behind incumbents
  appspawn(335)+ueventd(306), both pc=1; earlier incumbents at pc=0x101/0x102
  (SOFTIRQ bit + 1-2 — softirq-context monopolists, NOT 260-deep preempt
  stacks: pc=258 = 0x102). KOT-KILL small ones only; accountmgr never loaded.
  Killed at 747s; screen md5 82f0a25f (frozen logo class).
- lb56 (RELEASE main): SA inserts stop at 17 (17.8s); 105 DEADLOCK prints from
  6.3s — binder::WORLD x29 + UNIX_TABLE x79 (+1 beside sys_openat). Same
  family as lb48/50/51/52. Killed at 437s.
- Conclusion: the preempt-hunt spinlock fix did NOT cure the post-merge
  wedge; lb47b (debug clean) now looks like the coin flip, not a
  debug-vs-release distinction. The preempt-hunt lane's own ph5/ph5b
  boots (TASK tail) ALSO wedged on UNIX_TABLE — the wedge predates the
  spinlock fix. Wedge-fix lane's suspect set stands (977adb51/e1e7d8f9...).
- NOTE for wedge hunters: lb55/56 DEADLOCK ra resolves inside
  RawSpinlock::lock itself (deadlock_warn inlined; caller_ra = return
  address after the warn call) — the LOCK address is the informative
  field, not ra.

### The ladder rig (built this session)
- Branch oh-ladder = origin/evidence-premerge + cherry-picked 5f0537a8
  (sync: bound the preempt-disabled spin in lock/read/write) = 53f4fd47.
  Conflicts were comment-only + the preempt_disable_at site-shadow call
  (leakorigin hook absent on this base) — resolved to plain preempt_disable.
  bzImage saved: run/bzImage-ladder-53f4fd47 (release, 12.9MB).

### lb57 (ladder rig, release+spinlock fix): the healthiest fail ever — POISON EVENT CAPTURED
- SA storm COMPLETE (78 inserts, 0 DEADLOCK), accountmgr alive ALL BOOT (SA:200
  @174.3, NO group-suicide — KOT-KILL lines 173 -> 35 vs lb54), CreateNewUser
  222.9->262.9 end, lockstatus_100 SET 263.1. SLOW-TXN n=27 p50=27.5s max=72.9s
  (variance-dominated; still the samgr code-7 30s family).
- 263.218: same one-shot fail as lb54 — BUT WITHOUT any watchdog kill or restart:
  startUser -> "userId not exist" (2097193) 0ms apart, zero kernel traffic.
- THE POISON MOMENT (bracketed to the millisecond): 263.000/263.001
  `IPCObjectProxy PrintErrorDetailedInfo: failed, handle:26 error:32
  desc:*.IOsAccount proxy:4920816` x2 — error 32 IS ERR_DEAD_OBJECT
  (c_utils errors.h: BASE_ERR_OFFSET+EPIPE). SendRequestInner marks the proxy
  dead (`status == ERR_DEAD_OBJECT -> SetObjectDied(true)`) AT THIS PRINT —
  every later osaccount query (CheckUserParam at 263.218 included) fails
  instantly and silently. THE PERMANENT MARK IS MADE HERE, NOT BY A DEATH
  NOTICE: no death notice fired for it (DRS/notice net silent), accountmgr
  alive, and the kernel SENDFAIL net did NOT fire -> the send returned +32
  WITHOUT any kernel error verdict.
- SOURCE AUDIT (oh-robot, this image's tree): the ONLY way a sync send returns
  +32 with zero kernel traffic is HandleReply's TF_STATUS_CODE leg
  (binder_invoker.cpp:1026): a BR_REPLY whose flags carry TF_STATUS_CODE (0x08)
  has its FIRST INT32 read as the send status. Servers author that shape via
  BinderInvoker::SendReply -> WriteTransaction (binder_invoker.cpp:1241):
  an error return with an EMPTY reply parcel becomes TF_STATUS_CODE + errcode.
  => A TRANSIENT server-side error 32 from accountmgr's osaccount stub
  PERMANENTLY death-marks foundation's IOsAccount proxy. Kernel BR_DEAD_REPLY
  (raw 0x7205) does NOT mark (verified against this tree's invoker source);
  kernel ioctls return -errno, also not +32.
- Our kernel relays reply flags verbatim (construction flags:tr.flags,
  delivery flags:t.flags) and never fabricates status-code replies — the 32
  is authored by the SERVER userspace. TAP LANDED (oh-ladder @ +1):
  STATUSCODE-REPLY WARN on every BC_REPLY with TF_STATUS_CODE + nonzero
  status (names server pid/tid, code, poison value). lb58 runs it.
- OH-side: the fix belongs upstream (a stub error must not equal ERR_DEAD_
  OBJECT, or SendRequestInner must not treat HandleReply status as liveness);
  kernel-side there is nothing to fix on this leg — but the SERVER error's
  own origin (why accountmgr's osaccount handler returns 32 under load)
  is the next hop, and THAT one is perf-family.

### lb58: a REAL KERNEL BUG found and fixed — pcp refill race (ON MAIN TOO)
- lb58 (ladder rig + first STATUSCODE tap) panicked at 133.8s:
  `PANIC: index out of bounds: the len is 32 but the index is 32
   Location: kernel/src/mm/pcp.rs:153` under net-softirq alloc pressure
  (stack: ipv4 defrag dispatch -> unix registry path). Console dead after.
- Root cause: alloc_order0's miss check and its refill STORE run in two
  SEPARATE irq-off windows; a nested irq-context alloc between them
  refills the row on its own, so row.count is not the zero seen at the
  miss — the unconditional store then indexed past list.len() (PCP_HIGH
  = 32). Classic check-then-act across critical sections.
- FIX (oh-ladder): cap the stored frames at remaining-room-minus-one
  (never park count exactly ON PCP_HIGH — that would arm the next free
  push), surplus batch frames returned via the zone free path
  (INV-PCP-2 net-zero preserved). Same file is IDENTICAL on current
  main (md5-verified) — coordinator should cherry-pick the oh-ladder
  commit to main (mm guarded domain; needs its perf-gate rows).
- Also on lb58: the first tap iteration burned its 200 budget on
  samgr's boot-time status-7 storm in ~3s (samgr answers hundreds of
  GetSystemAbility-null ERR_NULL_OBJECT status replies while SAs load);
  budgets split now (poison-32 gets its own 400).
- Poison(32) sightings on lb58 BEFORE any osaccount event: samgr itself
  sent a status-32 POISON reply at 4.342 (tid 332), and device_manager's
  IOsAccount proxy (handle:3) was marked at 133.17 right after an
  accountmgr reply. The osaccount 32-authors are multiple processes.

## EIGHTH-SUCCESSOR STATUS SNAPSHOT (for the next successor / coordinator)
1. RIG: use oh-ladder (evidence-premerge + 5f0537a8 + taps + pcp fix),
   release bzImages saved in run/ (bzImage-ladder-*). main release AND
   debug both stall/wedge on the OH image (lb55/lb56) — UNIX_TABLE /
   binder::WORLD DEADLOCK family, pre-existing, NOT cured by the
   spinlock fix.
2. lb57 (the healthy-boot shape): accountmgr SURVIVES with the spinlock
   fix (no watchdog suicide); the ladder now dies ONLY at the osaccount
   proxy poison: a TF_STATUS_CODE reply carrying 32 from the SERVER
   side death-marks the caller's IOsAccount proxy permanently
   (SendRequestInner marks on status==32; verified in the image's own
   ipc source). Server-side authors are OH userspace (kernel relays
   reply flags verbatim; kernel BR_DEAD_REPLY/errno never produce +32).
3. NEXT: lb59 (sctap2+pcpfix kernel) — read STATUSCODE-REPLY POISON
   lines at the ladder moment: names the exact server pid/tid and time;
   correlate with accountmgr hilog to find which osaccount handler
   returns ERR_DEAD_OBJECT (OH bug: transient server error -> permanent
   client proxy death). If the server-side 32 is rare on a healthy
   boot, the ladder may complete by luck: watch for appspawn
   com.ohos.launcher + pixel md5 change; report IMMEDIATELY.

### lb59 — THE LADDER'S REAL ENGINE DECODED (SwitchToUser reached; silent empty user)
With the pcp fix + tap kernel (healthy boot, 76 SAs, 0 DEADLOCK/panic):
- LADDER ADVANCED PAST THE OLD WALL: CheckUserParam PASSED (no osaccount
  poison at the decision on this boot), foregroundUserId printed,
  SwitchToUser(old=0,new=100,isAppRecovery=1) EXECUTED at 251.6 —
  further than any boot in campaign history.
- Then the lb40 shape live: `ImplicitQueryInfoByPriority: can't find
  target AbilityInfo` x154 + `waiting query highest priority ability
  info completed` x151, spinning from 251.6s past 900s (no exit).
- ENGINE (source + log bracketed):
  1. CreateNewUser(100) start 189.054, end 218.643 (29.6s) — and the
     binder call itself is SLOW-TXN txn14922 accountmgr->foundation
     exec 29610ms.
  2. The handler NEVER printed `create user:100 bundle size:N`
     (LOG_NOFUNC_I INFO — INFO lines DO flow through the hilog tap:
     "startUser in service" I0 came through; key 'bundle size:' is in
     AMS_KEYS and the budget was alive at [274]) → the line never ran.
  3. OH source (bundle_user_mgr_host_impl.cpp): OnCreateNewUser's
     early leg — if GetAllPreInstallBundleInfos(...) returns FALSE:
     LOGE + `return ERR_OK` — CreateNewUser still prints "end" SUCCESS.
     That function's ONLY other false: `return !preInstallBundleInfos
     .empty()` — the filtered preinstall set came out EMPTY.
  4. The set comes from dataMgr->GetAllPreInstallBundleInfos() ->
     LoadAllPreInstallBundleInfos -> preInstallDataStorage_ (the
     distributeddata KVS). bootevent.bms.main.bundles.ready only set at
     217.2s — the first-boot scan was still running when CreateNewUser
     read the store. Under our kernel the KVS read inside the 29.6s
     exec never produced records (the ~30s exec = the KVS timeout
     class; 5th successor's distributeddata KV coin-flip, same family).
  => USER 100 IS CREATED WITH ZERO BUNDLE RECORDS. The launcher's
     AbilityInfo cannot exist for user 100; the AMS isBoot retry ladder
     (no attempt limit) spins forever. Screen stays frozen logo.
  5. Image-side verified (debugfs on system-lb2.img + hap unpack):
     Launcher.hap's home ability is an extensionAbility
     (com.ohos.launcher.MainAbility, type service, action.system.home),
     module.json declares NO u1enable -> NOT skipped by the U1 filter
     (U1==1 anyway) — the miss is the EMPTY SET, not the filters.
- PIXEL PLAY (in flight): a WARM /data. Real devices boot this way
  after first provisioning: boot once on a writable copy
  (userdata-warm.img, NOSNAP=1 added to lb-boot.py) so the first-boot
  scan PERSISTS the pre-install records; the next boot's CreateNewUser
  reads a populated local store (fast path, no first-population) ->
  per-user install stream runs -> AbilityInfo for user 100 exists ->
  appspawn spawns com.ohos.launcher -> pixels.
  lbwarm (population boot) running at write time; lb60 = warm+snapshot.
- GATES for the pcp fix + taps: make test ALL PASSED (lb59-maketest.log);
  x86_64 release + riscv64 + aarch64 builds rc=0.

### The 30s CreateNewUser exec — dissected per user directive (bug, not perf)
Two independent samples (lb59 tid 912, lbwarm tid 930 — same window, same
image state) agree exactly:
- The handler parks in sys=202 (futex) with a1=0x80 (FUTEX_WAIT |
  FUTEX_PRIVATE_FLAG — process-INTERNAL sync, NOT a binder reply wait:
  a binder client parks in ioctl sys=16 as the neighboring samples show).
- The uaddr chain is DETERMINISTIC across boots: 0x7fffffffc8f4 -> c844
  -> c654, each held ~10s (2 audit samples) — a CHAIN of three
  sequential ~10s stack-based waits (condvar/future wait_for), NOT one
  5s loop: phase1 c8f4, phase2 c844, phase3 c654, then give-up-empty.
- ZERO installd install-stream traffic in the window (only 2 tiny SACB
  oneways to installs) -> the per-user install loop never ran; the
  early-empty-set leg is confirmed from the kernel side too.
- The same 1Hz audit names the wedged SERVERS concurrent with the
  window: samgr code-7 handlers at 30-55s exec whose server threads sit
  in sys=45 (brk!) for the whole window — an ALLOCATION storm inside
  samgr's handler, plus netmanager->samgr 54.7s. Working hypothesis
  (lb60 proves): foundation's KVS-open needs a worker (FFRT/IPC pool);
  the pool is exhausted (workers parked as sync-IPC CLIENTS against the
  30s samgr handlers); the open never runs; main thread wait_for x3 x10s
  gives up -> silent empty user 100 -> no launcher, forever-spin ladder.
- TAP LANDED (oh-ladder): one-shot dump_all_tasks("txn-hang-20s") when
  any txn's exec crosses 20s — prints EVERY task's state + parked
  syscall mid-hang. lb60 = fresh-data boot with this kernel
  (bzImage-ladder-hangdump): the dump names the leaf's producer thread.

## TENTH-SUCCESSOR SESSION (2026-11-01 18:00+, branch oh-ladder @ af5577c5)
Mission: verify the poison-patched image end-to-end. The predecessor's
lbpop2 (population, NOSNAP) + lb64/lb65 (snapshot boots, system-lb3.img +
userdata-warm.img) got FURTHER THAN EVER — and then froze on a NAMED engine.

### lb64/lb65 decode (evidence: oh-lb64/65-serial.log + task dumps)
1. The empty-bundle wall is dead and the poison ladder advanced:
   lb65 `CreateNewUser user(100) start` @40.2 -> `create user:100 bundle
   size:34` — the per-user install stream RAN (lb64: SetBundleQuota for
   /data/app/el2/100/base/com.ohos.launcher @119.78s; hmdfs/sharefs
   launcher mkdirfails @119.3 = install stream touching the bundle).
   systemui + render_service (llvmpipe/CompThread) SPAWNED on lb64/65 —
   the GUI stack is loading.
2. THE WALL: phantom-preempt monopolists own every CPU and the tick-side
   PREEMPT-HEAL never fires (ZERO PREEMPT-HEAL lines on both boots; lb59
   pre-contiguity-bar fired 1784). lb65 dump @64.5s: cpu slots 1232
   (m.ohos.systemui, preempt=0x10000) / 1067 (render_service watchdog,
   0x100) / 319 / 1177 (tgid 313 OS_IPC/FFRT, 0x10000) — all four busy,
   nr_running=165; installs tid 1138 state=RUNNING on_rq=1 NEVER picked
   (its mkdir = txn13201 exec 2350s; the CreateNewUser txn9711 itself
   2355s). lb64: systemui main wedged in exit_group (lastsys=231) at
   0x100 from ~118s through kill @660s.
3. Root cause: d2381bf9's contiguity bar re-arms the heal window on any
   completed PREEMPT pair — correct for the base field, but it shields
   stranded SOFTIRQ/HARDIRQ bits on lock-hot carriers (OS_IPC threads
   live in spinlock loops), AND the pure-field faces (base==0) were
   never reachable by the base!=0 gate (keep_tick_nesting existed but
   had NO call site). preemptible()==0 for ANY bit, so these monopolize
   exactly like the base face.
4. FIX (af5577c5): per-field contiguity — Task ti_pc_sirq_zero_jif /
   ti_pc_hirq_zero_jif stamped whenever the tick observes that field
   ABSENT; a field present at 200 consecutive ticks is phantom (lock
   traffic never touches these fields). Stale field -> skip the re-arm
   -> window matures -> new heal leg (base==0): keep_tick_nesting +
   need_resched + PREEMPT-HEAL-FIELD print with softirq/hardirq origin
   sites; the base leg now collapses stale hardirq extras too.
   Base-face (pc=1) policy deliberately untouched.
5. Build note for successors: `build-release` does NOT regen ksyms
   (only the debug `build` target does). A release build after a
   `make build` for ANOTHER arch embeds wrong symbols. Two-pass release:
   cargo build --release; gen-ksyms.py <elf> x86_64-unknown-none
   kernel/src/dfx/ksyms_generated.rs; cargo build --release; mkbzimage.
   LTO byte-loop-transforms the tick prints into immediate stores —
   `strings` on the release ELF is NOT evidence a print was stripped;
   disassemble for the heal's andl sites instead (0xff0000 = clear_base,
   0xf10000 = keep_tick_nesting).
6. lb66 (verification boot): bzImage-ladder-fieldheal + system-lb3.img +
   userdata-warm.img + dfx=ams46,txnhunt, SMP=2. Watch: PREEMPT-HEAL*
   lines (the fix firing), the install stream completing (installs
   code-4 execs bounded), startUser in service:100 -> CheckUserParam
   (poison leg: POISON32-DELIVER then NO marking = patch works) ->
   SwitchToUser -> appspawn com.ohos.launcher -> pixel md5 change
   (frozen-logo set: 5f8b704d/06b933b4/82f0a25f/23312e5b).

### Tenth-successor session close (2026-11-01 ~19:45, branch @ e8b9a969)
1. HEAL STACK, three rounds, honest final form (8387c00b): per-field
   tick-observed absence stamps for ALL THREE count fields
   (ti_pc_{p,s,h}irq_zero_jif); a field PRESENT at 200 consecutive
   ticks is phantom, a field observed ABSENT refreshes its stamp. This
   replaces both the sc42 cumulative window (false-healed live
   softirq windows — round 2's make-test wedge) and d2381bf9's
   PREEMPT-pair re-arm (shielded stranded S/H bits behind lock churn —
   the lb61-lb66 zero-heal freeze). make test GREEN 1267/1267 (the
   VW-WEDGE prints after "All tests passed! Halting..." are POST-HALT
   writeback noise, NOT failures — verify the suite verdict line before
   believing a wedge; 3112b576 also passes, sometimes without the noise).
2. THE 441s INSTALLS STALL, decoded to its wait site (coordinator
   redirect; lb66 tid984 stack, symbolized against the af5577c5
   two-pass rebuild): sys_mkdir -> vfs_mkdir -> ext4_mkdir_wrapper ->
   ext4_journal_stop (the mkdir's journal commit) -> blkdev_write ->
   submit_request -> VirtQueue::wait_for_used_interruptible — the
   POLL path (caller holds a preempt guard: the VFS mutation lock over
   the journal write), 5M laps, NO deadline (poll_only skips the 10s
   deadline entirely!), caller retried for 441s+. NOTE: Rux's internal
   dispatch uses asm-generic numbers: nr 83 = FDATASYNC; the x86 native
   83 = mkdir maps to internal 34 — the lastsys=83 prints on the mkdir
   stacks are the NATIVE number. The wait site is the same either way.
3. Instruments landed for the capture (e8b9a969): VW-POLLSTALL (the
   poller's own ring snapshot at 2M laps: resp byte, used idx,
   prev_used, pc — splits delivery-loss vs kick-loss vs descriptor-
   window in one line) + re-armable txn-hang-20s dump+vw_report (the
   old global one-shot latch spent itself on an early 20s samgr txn
   and went dark at the real hang — lb70 lesson; now once per 600s).
4. OH boots this session (all bzImage + system-lb3.img + userdata-warm,
   dfx=ams46,txnhunt, SMP=2): lb66 (r2 kernel) froze at the monopolist
   wall AGAIN pre-install; lb67 (cumulative heals, 185 heals/70
   underflows) passed the wall, SA storm 88, then the accountmgr
   userspace-futex stall (txn16360 code66, tid977 in FUTEX_WAIT with
   txn16423 QUEUED behind it) — the fifth successor's pool-exhaustion
   family, NOT kernel scheduling; lb70/71 (r3 heals, 34-119 heals,
   28-60 underflows at spinlock.rs:210 — the samgr-brk/mm-unmap real
   2s+ hold false-heal cost) stalled pre-install (lb71: accountmgr
   txn19304 code53 exec 445s). The wall ROTATES: monopolists /
   accountmgr futex / samgr code-7 storms / installs fdatasync —
   boot-to-boot variance decides which one bites.
5. NEXT (in order): (a) roll boots until one reaches the install
   stream with the r3i kernel — VW-POLLSTALL names the fdatasync
   engine directly; the fix is then targeted (candidates: give the
   poll_only path a deadline + kick+walk like the sleep path's; the
   caller retry loop in submit_request; the descriptor window). To
   reproduce lb66's no-heal shape exactly, boot 3112b576 + cherry-pick
   e8b9a969. (b) The accountmgr futex family (tid parked with queued
   txn) is userspace-internal — the samgr/installs 30s chain on the
   campaign map. (c) The r3 heal false-positive cost on REAL 2s+ holds
   (spinlock.rs:210 underflows) wants the leakorigin outermost-age
   discriminator if it grows.
6. Gates this session: x3 builds rc=0 (x86_64 release two-pass +
   riscv64 + aarch64), make test GREEN (lb69-maketest.log: verdict
   green, 1267/1267). Branch pushed through e8b9a969.

## THIRTEENTH-SUCCESSOR SESSION (2026-11-02 03:30-08:30, branches oh-ladder @ 79cb96cb + biofix-fieldpoll @ 6e5afaae)
Mission per charter: population race -> pixels. Coordinator redirected mid-session
(07:00): STOP image patches, hunt the kernel stall. Outcome: THE 550-1100s
STORE-LOAD STALL IS DECODED AND FIXED; a second, OLDER blocker (the allocator
freeze) is now named to its exact victim signature and blocks verification.

### The bio write-plan closure burn (root cause of the crawl's 550-1100s loads)
1. lb229 evidence (252 identical 5s TXN-HANG samples): distributeddata's
   OS_IPC_0_912 frozen at fs::bio::WritePlan::push_entry for the WHOLE 1092s
   window; bundles.ready landed 1091.6s. The kernel stack never changed.
2. Engine: plan_add_constraints (fs/bio.rs) rescans EVERY pending publication
   against the whole plan per round; the plan grows a few blocks/round ->
   thousands of quadratic rounds = 18+ min kernel CPU in ONE fsync/mkdir
   durability point. Every service pays it behind accumulated churn; the KV
   load (many fsyncs) crawls 550-1100s vs ~40s healthy. This is the O(batch^2)
   class the fs lane fixed for jbd2 — the SAME family in the bio WritePlan.
3. Fix iteration 1 (index+worklist closure) ALLOCATED from journal/eviction
   spinlock contexts -> allocator wedges (lb234/235: samgr workers frozen in
   RawVecInner::finish_grow; REVERTED). Fix iteration 2 (79cb96cb, FINAL):
   allocation-free round cap (3 rounds) + conservative linear fallback
   (schedule every record's pres+entries for the device — strict superset,
   same pre->entry phase discipline, one linear pass). Gates: make test ALL
   PASSED; x3 builds rc=0 (x86_64 two-pass release, riscv64, aarch64).
4. Also landed while chasing the dark family: FXDV2 param-arena futex tracer
   ran ALWAYS-ON (500 console prints from do_futex's hot path in the first
   seconds of every boot) — gated behind dfx=futextrace (6e5afaae).

### THE REMAINING BLOCKER (older than this session, now precisely named)
Every boot since ~07:45 wedges before SA inserts (samgr-dark; 10+ consecutive,
all kernel classes/all images; ~50% flowed earlier tonight; load-correlated).
Victim signature EXACT: parking threads sit in unix_write_slice_owned ->
RawVecInner::finish_grow (a HILOG write's allocation), epoll_create1, or
finish_grow directly — i.e. every allocating syscall parks on the small_alloc
class lock held by a thread frozen mid-section. That is the mm guarded-domain
PHANTOM-PREEMPT-INSIDE-ALLOCATOR family (leakhunt's named acquire sites:
small_alloc.rs:283, slab.rs:628, buddy_allocator.rs:857). NOT kernel-class
specific, NOT my bio fix (allocation-free now), NOT FXDV2 (gated, still dark).
This freeze ALSO explains the population boots' bimodality all along.

### Verification state (blocked)
run/verify-roll.sh (bzImage-ladder-fx2fix = fieldpoll+biofix+futexgate,
system-lb5 unmodified, warm snapshot) re-rolls for a boot that reaches the
store-load window: measure bundles.ready vs the 550-1100s baseline; expect
the fast (~40s) class. Kernels in run/: bzImage-ladder-fx2fix (verification),
bzImage-ladder-biofix{,2} (superseded iterations).

### Population-path decodes (committed for the successor, image work stopped)
1. Creation trigger (OH source): accountmgr's OnAddSystemAbility runs
   CreateOsAccount(100) once storage(5003)+AMS(180)+BMS(401) SAs registered;
   subscription via samgr's CheckListenerNotify (late subscribers notified).
2. User-100 resurrection: GetOsAccountInfoById falls back to the
   distributeddata KV when the file is absent — stripping only
   /data/service/el1/public/account is INSUFFICIENT; the store lives at
   /data/service/el1/public/database/os_account_mgr_service/ (discovery boot
   lb216 listed the paths). IsOsAccountExists = presence of
   account/<id>/account_info.json (valid JSON).
3. Race margins (creation start vs bundles.ready): lb59 -28s, lbpop2 -239s,
   lb219 -38s, lb226 -0.4s (read at exec start ~155s loses), lb229 lost by
   12ms (retry budget 900s burned, ready at 1091.616, giveup 1091.604). With
   the bio fix compressing the load, the natural race should win on a
   healthy boot. Image-side: system-lb6/8 (strip stubs), lb9 (condition,
   bypassed by dynamic SA loads), lb10 (ondemand flips), lb11-13 (patched
   libbms retry) — ALL SUPERSEDED, do not boot; system-lb5 remains the
   campaign baseline; lb7 = discovery (prints paths to console).
4. FIRST MOVE when the allocator freeze is fixed: verify the bio fix
   (verify-roll), then the strip-population (system-lb8 recipe: strip account
   dir + os_account_mgr_service dir at 4.3s via sdc stub, NOSNAP fresh warm
   copy) -> creation reads the populated store -> bundle size:34 -> install
   stream persists -> GOLDEN copy -> snapshot ladder -> pixels.

### Boots this session
lb200 (roller aborted), lb210 (strip v1: KV resurrection), lb216 (discovery),
lb217-231 (golden rolls: samgr-dark mix + race losses; lb219/222/223/225/226
created user-100-empty = copies deleted; lb227/229 = the retry-budget
boundaries), lb232-245 (verification attempts: the allocator-freeze dark
streak). Gates: make test ALL PASSED (biofix-maketest-full.log), x3 rc=0.

### Why the heal machinery never fires during the allocator freeze (for the sched lane)
The frozen class-lock holder sits inside lock_irqsave (IF=0 + preempt off):
its OWN CPU's tick cannot fire, so every tick-side observer (PREEMPT-HEAL
per-field stamps, claim-sweep census) is blind to it — ZERO heal lines on
every dark boot while the machine is wedged. Only an observer on the OTHER
CPU (GRQ timeline walk / cross-CPU dump) can see and name the holder. A
holder-naming DFX needs to be cross-CPU by construction; also note the
waiters cannot self-report (printing allocates — they are parked IN the
allocator). Boot-side, the frozen holder's identity is recoverable from a
cross-CPU task dump (the txn-hang dump_all_tasks machinery) — none armed on
the dark boots (the 20s-latch had not fired before the wedge).

## FOURTEENTH-SUCCESSOR SESSION (2026-11-02 10:45-13:00, branch biofix-fieldpoll @ 0508ff20+)
Mission: verify the bio fix end-to-end + hunt the allocator freeze. Outcome:
THREE RIG BUGS and TWO KERNEL PANIC CLASSES fixed; the "freeze" did NOT
reproduce on the quiet host — tonight's reproductions were INSTRUMENT
RACES plus the roller killing healthy boots. The allocator itself
measures at MICROSECOND class. The pixel wall is back to the
store-load/ladder userspace family.

### Rig bugs found (these poisoned the 13th's dark-streak evidence)
1. verify-roll.sh READY grep matched the NEGATIVE "not set" warning line
   (lb246's false READY at 169s = foundation exit; fixed: positive
   marker only).
2. verify-roll.sh never set USERDATA -> every verify boot (lb233-lb246,
   the WHOLE dark streak) mounted the checksum-poisoned userdata-fresh
   symlink: /data mount FAILED (-5, superblock checksum invalid). The
   13th's "freeze" boots were entangled with the no-/data trap (the
   fourth successor's lb16-lb26 lesson). Fixed: USERDATA pin. Baselines
   lb229 etc. were gold-roller reflink copies (vdf mounted rc=0), so the
   550-1100s class numbers stand; the DARK-BOOT diagnosis does not.
3. poproll.sh killed the VM the moment an ALLOC-* print appeared —
   combined with (see below) a racy stamp table, healthy boots were
   murdered at 29-360s and read as "the freeze kills the machine".

### Kernel fixes (committed, pushed; make test green x3; x3 builds rc=0)
1. mm/pcp.rs: free_order0's entry store bound-checked (lb250 PANIC
   "index 32" at 22s: the 35336eb1 refill guard can leave count ==
   PCP_HIGH exactly; a full row + this store = out of bounds. Full row
   now spills oldest batch + sends the incoming frame straight to
   buddy).
2. Print-path PANIC class (lb266 @209s): from_utf8_unchecked on task
   comm (userspace-writable via prctl) panicked inside the KOT-KILL
   group print's Debug fmt ("end byte index 13 not a char boundary")
   during do_exit_group — one print killed the kernel. Fixed: checked
   UTF-8 at all 10 comm sites (exit.rs/signal.rs) + netdev get_name
   (SIOCSIFNAME names). lb250's StrSearcher-shaped trace = same family.
3. mm/small_alloc.rs: both class page-list walks capped at 8192 pages
   (cycle defense; walk_cycles counter). DEFENSIVE — no cycle was ever
   observed (see meter below).

### The allocwatch DFX (dfx=allocwatch) — and the mirage it created then killed
- Final design: DEDICATED per-key slots (site base + class; 24 slots),
  cross-CPU tick scan reports holds >300ms with holder identity/state/
  preempt/ti_cpu/buddy-word + kernel-stack PCs; ALLOC-STUCK lock-word
  detector (200ms); per-class slow-path section meter (max TSC cycles,
  >10ms count, 10s heartbeat).
- Iterations 1-3 used a SHARED 16-slot scan table — two different class
  locks' enters raced the slot choice and MIXED pid/epoch/jif between
  stamps; the mixed stamp survived its exit and read "held 300ms-1s".
  The prints named real pids (os_ds_service) that were NOT stuck.
- The meter is the arbiter: healthy-boot sections measure 76-1500
  CYCLES (<= 0.6us), over10ms=0 all boot. NO slow section ever existed.
- lb253 (the one boot never killed on a print) ran its FULL 1500s
  healthy with /data mounted (userdata-warm2). The 13th's
  finish_grow-park freeze is HOST-LOAD-CORRELATED and did NOT bite in
  single-agent quiet-host conditions (tonight's host: 3 lmbench loops
  from other lanes' worktrees, load 7/32, VM at 199% = both vcpus
  healthy).
- ABBA hypothesis (coordinator ask): STRUCTURALLY ABSENT — small_alloc
  class sections are pure mm (alloc_locked -> buddy only; no
  fs/journal/reclaim calls; buddy sections O(1) helpers, bounded
  loops). The REVERSE nesting EXISTS (bio LRU lock -> Box free ->
  class-lock flush) = latency amplifier only, not a cycle.

### The bio fix's end-to-end measurement (honest numbers)
- lb253 (fx3fix kernel = fieldpoll+biofix+pcp1, system-lb5,
  userdata-warm2, /data rc=0): bundles.ready NEVER SET in 1500s; ladder
  spun (186 waiting-query). The WritePlan burn is GONE (tid877's stack
  = nanosleep/poll retry ladder, userspace-internal), but the window
  did NOT compress on warm2's shape. The 550-1100s "class" measured by
  the 13th was on STRIPPED warm copies — warm2 (unstripped, stale
  user-100) is a DIFFERENT, slower shape. No clean A/B landed yet.
- POPULATION STATE (lb260/lb275, warm.img): startUser at 101s with NO
  CreateNewUser — warm.img ALSO carries a stale user-100 (the ninth's
  NOSNAP lbpop wrote it). Both warm images have the
  user-100-per-user-records question open; the ladder misses
  (ImplicitQueryInfoByPriority) on every boot. PIXELS need userdata
  with NO user-100 + populated store: the strip recipe (image-side,
  FORBIDDEN now) or FRESH provisioning (userdata-fresh2.img — creation
  retry budget vs first-scan; the bio fix should flip it; UNTESTED
  this session).

### Boot inventory this session
lb250 (pcp panic 22s), lb251/252 (dark, poisoned userdata-fresh),
lb253 (fx3fix, warm2, full 1500s, healthy, no ready), lb260 (fx4fix,
warm, ALLOC print @360s = stamp race + roller kill), lb261 (same @29s),
lb262/264 (not-found stamp face), lb263 (fx5fix, stack=creation-sp,
freeze print @301s), lb266 (fx6fix, UTF8 PANIC @209s), lb267 (killed
for fx7), lb268 (fx7fix, 300ms-stamp w/ clean preempt+buddyword ->
meter demanded), lb270 (fx8fix, meter live: 1500 cycles), lb275+
(fx9fix, clean instrument, no-kill roller — ROLLING at close).

### FIFTEENTH-SUCCESSOR ENTRY POINTS (in order)
1. lb275+ verdicts (pop14.log): with the no-kill roller these boots
   LIVE — read bundles.ready timing, the ladder outcome, and any
   ALLOC-* prints (now trustworthy). If the ladder spins on warm.img's
   stale user-100 (expected), switch the roller's USERDATA to
   run/userdata-fresh2.img (fresh provisioning — the kernel-only pixel
   path) and watch the creation-vs-ready race.
2. The store-load window: instrument-decode WHERE the 1500s+ goes on
   warm2 (lb253's tid877 nanosleep ladder — the distributeddata retry
   chain; SLOW-TXN census on a live boot names the servers in its
   wait chain). This is the bio-fix A/B that never landed.
3. The 13th's freeze: reproduce by LOADING the host (the correlation
   is the clue) with allocwatch armed — the fixed-slot instrument + the
   meter will now name a real holder (if any) vs host starvation. If
   NO stamp ever fires under load, the freeze is vcpu starvation or a
   stuck word (ALLOC-STUCK) — the mm fix follows from which.
4. Gates reminder: everything is on origin/biofix-fieldpoll; the mm
   guarded-domain rows (pcp, small_alloc) need syscost/pbench numbers
   before main-merge; the UTF8 fix is print-path-only (no gate).

### FOURTEENTH-SUCCESSOR ADDENDUM — the coordinator's ext4-biglock steer, decoded
1. (a) THE HOLD HYPOTHESIS IS DEAD: maxheld instrument (release-side
   max hold + holder) reads **4 jiffies (40 ms)** as the boot maximum —
   NOBODY holds the biglock for 5 s. The waited=500j episodes are QUEUE
   STARVATION: the anti-starve timer (5 s) is the only visible episode
   length. STALL-FREE lines (owner=none, waiters=5) show a FREE lock
   with parked waiters — the handoff wake fires but the woken waiter
   cannot be SCHEDULED: phantom-preempt monopolists own the CPUs (the
   campaign's open producer, Face B). The queue protocol itself is
   sound.
2. FAIR-SPINNER FIX LANDED (real mitigation): task-context preempt-bit
   callers on lock_fair previously degraded to the UNGATED barging
   acquire — a phantom-bit chmod walker (chmod_ros pid 524 storm)
   barged past every parked waiter per op. Now they spin on the GATED
   acquire (2 s BARGE_ESCAPE fallback). Stall count unchanged
   (lb275: 7 vs lb280: 6) because the root is scheduling, not barging.
3. (b) SENDFAIL taps: the storm onset (80.3-81.6 s) shows samgr REPLY
   failures — why=6-reply-empty-stack, why=17-reply-target-gone — then
   why=3-owner-proc-dead (pid 333->handle 23): REAL process deaths
   after the stall era (OH watchdogs firing), so the poison is a MIX:
   real deaths + late-reply/empty-stack on stalled callers.
4. (c) SA:401 (BMS) DID insert (and SA:4010); bundles.ready=0 is the
   BMS scan never completing (the store-load wall), not registration.
5. NET: the poison storm's kernel-side window is the STALL ERA
   (5-78 s) = scheduling starvation under phantom monopolists. The
   producer hunt (r83hunt / preempt bits) remains THE wall; the
   biglock fairness fix removes one amplifier. EXT4-BIGLOCK-HOLDER-STK
   + maxheld instruments are in for the next stall read.

### FOURTEENTH-SUCCESSOR ADDENDUM 2 — monopolist snapshot + the storm's true author
1. Snapshot instrument (fx11/fx12fix): dump_all_tasks at BOTH stall
   variants (free-strand + locked-churn) + the waiter's explicit
   on_rq/preempt/ti_cpu line. lb285 (fx11) ran the CHURN shape
   (maxheld=3j) with NO free-strand — snapshot moved to both branches.
2. lb290 (fx12fix, 445s): the POISON STORM HIT WITH ZERO BIGLOCK STALLS
   — 2372 POISON32 lines, no STALL prints at all. The stall->storm
   causality is NOT universal.
3. THE STORM'S AUTHOR IS NAMED: pid 961 = accountmgr (started 94.5s)
   authored 2389/2407 POISON32-DELIVER, victims = foundation (pid 370,
   tids 988/1203 x~600 each). This is the eighth-successor decode
   exactly: accountmgr's osaccount stub returns error-32 (ERR_DEAD_
   OBJECT as TF_STATUS_CODE), which PERMANENTLY death-marks foundation's
   IOsAccount proxies → every osaccount query fails silently → no
   user-100 creation → no launcher. The 32's own origin = accountmgr's
   internal KV read failing under the store-load wall (the campaign's
   standing userspace-internal timeout family).
4. NET for the launcher: the chain is store-load wall -> accountmgr
   handler errors -> 32-poison of foundation -> creation dead. The
   kernel-side lever remains the STORE-LOAD COST (bio fix removed the
   WritePlan burn; the residual is the distributeddata/samgr retry
   ladder — fifth-successor territory), NOT the biglock (holds are
   3-4j; the fair-spinner fix landed as hygiene).

### FOURTEENTH-SUCCESSOR ADDENDUM 3 — the retry ladder's never-true condition, named
lb253's window (116-146s), kernel-side dependency chain from the 1Hz audit:
1. tid877 (distributeddata OS_IPC_1, pid 873) executes asset_service's
   txn12462 parked in sys=35 NANOSLEEP — the FFRT/DB_TaskExecutor
   wait_for (userspace-internal; the check between sleeps is not
   kernel-visible).
2. The FFRT pool worker tid874 (OS_IPC_0) is parked as a sync-IPC
   CLIENT in sys=16 against USERIAM tid 654, txn10152 code 12, exec
   5530ms -> 10530ms+ (the 30s-class exec family — crypto + syscalls
   under the per-op wall; useriam is the historical watchdog-kill
   victim on earlier boots).
=> NEVER-TRUE CONDITION: tid877's wait_for returns only when a pool
   worker frees; the pool's worker is blocked on useriam's code-12 for
   10-30s+ each; every dependent (asset_service, powermgr, drm_service,
   usb_host — 235+ hung txns each) queues behind distributeddata.
   Combined with the accountmgr 32-poison (addendum 2): accountmgr's
   own KV handlers ride the same pool starvation, time out, return 32,
   and permanently death-mark foundation's osaccount proxies.
=> KERNEL LEVER: useriam's exec cost = the per-syscall wall (the perf
   lane's p1_null 4.5us class x millions of calls) — cross-lane. No
   correctness bug in this hop; the kernel-side fixes this session
   (pcp, UTF8, walk bounds, fair-spinner) stand as hygiene + panic
   kills. Biglock stalls: 7 (lb275) -> 0 (lb290) with the fair-spinner
   fix in — recorded, variance-caveated (lb290 had no chmod-era
   contention at all).
lb290 bundles.ready: NEVER fired (87 SAs, storm on) — the store-load
residual is stall-independent, routing through the pool/useriam chain.

### SESSION CLOSE (14th successor, 2026-11-02 ~14:10)
Branch pushed @ 4420bb92+this. Rollers: poproll continues (fx12fix,
snapshot-armed; PIDs owned by lb-boot.py lb29X drivers — kill only
those). Merge queue: coordinator takes biofix-fieldpoll. Successor
entry points in the addenda above + the main close section.
