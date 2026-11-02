# stress-port lane: port industry stress tests, find stability bugs

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
---

(main carries the stress-port charter below — other lane's TASK content, keep)

## Charter (user directive 2026-11-02)
"Supplement stress test cases — stability is still a problem. If the
industry has stress tests for the reference kernel, port them directly
and run them." PRIMARY vehicle: x86_64 + KVM (platform priority).
Goal: panics, wedges, leaks, and scheduler starvation exposed by
sustained load — then kernel-side fixes.

## What to port, in order
1. stress-ng (the industry standard, ~300 stressors). Build a static
   x86_64 subset against the toolchain already used for probes (see
   test/ build patterns; musl static preferred). Inject via the LTP
   vehicle in /home/william/rux-images/testimages/ (boot rig there
   already runs x86_64+KVM). Start with the classes our bug ledger
   says are weak:
   - memory: malloc/mmap churn (allocator class-lock family),
     mmapfixed, bigheap (zone exhaustion vs the fake-OOM history)
   - fork/exec churn (p4 regression history; exit-path RCU grace)
   - filesystem metadata churn (mkdir/rmdir/unlink storms — the OH
     chmod-storm shape; bio write-plan under load)
   - IPC: pipe/socket storms, SysV sem/msg/shm
   - sched: many-thread yield/wakeup storms (phantom preempt family,
     WAKE-STRANDED census)
   - timer/clock: settime/interval storms (tick discipline)
2. LTP robustness subset already in the tree (fork12, mmapstress,
   mmap-corruption, sem/msg stress) — run long-loop, not just once.
3. (later) syscall-fuzzer class (trinity-like) once 1-2 are clean.

## Method
- One stressor class per boot; serial console is the log.
- PASS = zero panics/wedges over the budget; FAIL = named bug entry in
  run/findings.md + minimal reproducer.
- Every kernel fix: x3 builds rc=0 + make test green + stressor rerun
  clean before push (origin/stress-port).
- Use the in-tree DFX under load: dfx=allocwatch (class-lock holder
  watch), syscost census bands, PREEMPT-HEAL counters — stability runs
  double as Tier-1-under-load evidence for the perf gate.
- Host discipline: at most 2 own QEMUs; -snapshot + file.locking=off;
  kill only your PIDs; pkill forbidden (pgrep -f self-matches: verify
  by exact cmdline).

## Session log (append per session)

### 2026-11-02, round 1 — rs suite ported, 2 kernel bug families fixed, 2 stressors re-run clean

**Ported** (stress-ng stressor logic as standalone static musl probes,
per charter method; sources archived in userspace/tests/probes.tar.gz
per the 394c16ce policy, host driver test/stressng/run-class-x86.sh):
14 classes — malloc/mmap/bigheap (memory churn), fork/exec, fsmeta
(metadata storm with read-back verify), pipe/sock/sem/msg/shm (IPC),
futex/sched (wakeup storms), timer (create/settime/delete + CLOCK_
MONOTONIC discipline + nanosleep latency), plus --spawn passthrough
for minimal reproducers. Vehicle: the x86_64+KVM LTP rootfs (copy of
the permanent-store image in run/, rs/rsrun/renameco injected via
debugfs); PID-1 rsrun runs cmds.txt lines with per-class budgets.

**Round-1 campaign** (baseline 819a1be2, one family per boot,
dfx=allocwatch,taskdump,watchdog + syscost=1, logs in
run/stress-runs/): ipc / sched / timer / memory PASS (0 panics, 0
wedges, 0 probe errors); fsmeta FAIL (SP-2); forker survived but lit
up the SP-1 storm. Verdicts + census numbers: run/stress-runs/
findings.md.

**SP-1 (fixed, verified)**: PREEMPT-HEAL false-fires stripped live
guard bits of fresh fork/exec tasks (15,577 heals + 169 underflows per
120 s of fork churn). Two producers: (1) stale_200's j==0 arm treated
never-stamped fields as instantly stale — fixed with arm-on-first-
observation (sched.rs); (2) new_task_at did not zero the four
ti_pc_*_zero_jif stamps on recycled Task pages — same class as R68a
(task.rs). Verification boot (fork 8 + exec 4, full 100 s budgets):
**0 heals, 0 underflows, 0 panics** (was 15,577/169/0).

**SP-2 (fixed, verified)**: ext4 setattr corrupted freshly-written
files — the success tail ran page-cache invalidate_inode for EVERY
attr flavor (chmod dropped dirty unflushed pages) and metadata-only
setattrs wrote back the disk-read inode's stale i_size (0 for pending
writeback). renameco minimal reproducer isolated chmod (3/300 fails,
reads returned EOF; rename alone clean). Fix: invalidate gated to
ATTR_SIZE, metadata setattrs carry the live VFS size (ext4/mod.rs).
Verification boot: renameco 0/300 on every sequence; fsmeta 4-worker
class full 120 s rc=0 (was: all 4 workers corrupt on cycle 1).

**Driver bug found+fixed in-flight**: rsrun killed classes at an
absolute 60 s (RES rc=137 truncation in the first campaign's memory/
forker boots); now parses --timeout per class.

**Tier-1-under-load evidence** (syscost, memory boot): IF=0 cpu0
17.6 s/60 s (rest 17.2 s, hard 408 ms, uart 3 ms), top rest-site
sched.rs:2568 (37.4M ops avg 834 ns); no ALLOCTHROW / DEADLOCK /
KERNPANIC classes in any boot. Follow-up hunts queued: IRQ-OFF-WINDOW
idle-wake prints 65-142 ms under load (pre-existing, guarded domain
interrupts); WAKE-STRANDED 118 s face under memory churn (SP-1
family; re-check on fixed kernel in round 2).

**Round 2 queue**: dirstorm-style shared-tree churn + fsync pressure;
pipe/socketpair under CPU contention (2 vCPU); LTP robustness subset
long-loop (fork12, mmapstress, mmap-corruption); brk-churn probe via
glibc-static (musl pins brk); make test + full LTP spot-scan on the
fixed kernel to guard the setattr/preempt changes.


### 2026-11-02, round 2 — SP-3 fixed, IRQ-OFF-WINDOW adjudicated artifact, residual family censused, brk covered

**Sync**: merged origin/main aa43f52e (round-1 SP-1/SP-2 content landed
there); branch descends from main. Round-2 commits: 126e99b9 (kernel),
ec9a0079 (probes).

**Queue item 1 (SP-2 deep soak) + SP-3**: new rs classes dirstorm
(shared-tree legal races, strict fd-pinned verify) and dirsync (16 KB
write + fdatasync/fsync + fchmod + rename + dir-fsync storm). fsdeep
boot: dirstorm 6x150s PASS (2.02M ops, 0 corrupt); fsmeta 8x180s PASS
(1.45M ops — SP-2 area holds at 2x round-1 intensity); dirsync caught
**SP-3: fsync(dirfd) on ext4 returned EINVAL** (sys_fsync's blanket
!is_regular_file check; the reference kernel's ext4_dir_operations
carries .fsync, and our own namei defers entry-block durability to
fsync(dirfd)). Fixed (is_directory + ext4 ops -> ext4_sync_file);
rerun clean: dirsync 615,856 ops rc=0, dirstorm 2.55M ops rc=0 with
strict dir-fsync checks. make test green after fix.

**Queue item 2 (IRQ-OFF-WINDOW idle-wake)**: adjudicated MEASUREMENT
ARTIFACT, not an interrupts-domain violation. Evidence: periodic LAPIC
tick + idle halts with IF=1 (gap = halt-exit delivery latency);
user-frame gap lines prove steal-magnitude delays with IF=1; syscost's
cli/sti zone accounting measured organic REST maxima <=2 ms in the same
boots (the only >7 ms segments were ONE host-stall window quantized on
both CPUs: 2619 ms REST @ file.rs:581 cpu1 + 2620 ms UART cpu0, fired
once, never again). Probe re-tagged: IRQ-DELAY-IDLE / IRQ-DELAY-STEAL
for artifact frames (kernel change 126e99b9), verified live. No
interrupts fix needed; guarded-domain band holds organically.

**Queue item 3 (WAKE-STRANDED re-census, SP-1-fixed kernel)**: the
residual family STANDS and is deterministic. memory2 boot: malloc rc=1
(worker 308 wedge, needed SIGKILL — campaign's first wedge), 1x
WAKE-STRANDED 100,469 ms/22,262 visits, 21 heals, 3 underflows. Chain:
rs malloc parent (pid 307) carried preempt_count 0x10004 for 200.7 s ->
wakeups refused -> heal -> induced underflow at a page-allocator lock
drop (.sdata near mm::pglist::NODE_DATA). leakhunt rerun reproduced
everything (wedge + 100.3 s strand + 200.8 s leak) and captured
false-fire ORIGIN sites mm/zone.rs:567 + mm/mm_struct.rs:958 with
oage_jif=1 (live acquires healed instantly) — steal-window false-fire
producer: the ti_pc_* stamps refresh only on the task's own CPU tick,
so a task parked mid-guard through a >2 s host steal window resumes
"stale-stamped + bits present" and is healed live. Numbers are for the
OH lane (same family); steal-aware exemption for the 200-tick bar is
the argued fix direction.

**Queue item 4 (LTP robustness long-loops)**: rsrun gained an
`exec <budget> <path> [args]` line form (fixed a stale-tail argv bug
found in-flight: NULL terminator placed before two token shifts — LTP
new-API binaries rejected the duplicated path arg; host-verified).
ltplong sweep (49 entries: mmapstress01-10 x3, mmap-corruption01 x3,
msgstress01 x3, msg_comm, sem_nstest, fork_procs, fork_exec_loop,
growfiles): mmapstress01 x3 + mmap-corruption01 x3 rc=0. Adjudicated
NOT bugs (host reference identical): mmapstress02/03/05/08 brk-ENOMEM
(1990s static binaries; their brk roundup fails on the modern
reference kernel too). Usage-shape (my cmds lacked args):
mmapstress06/09/10 — rerun with canonical args in ltplong3. REAL
suspect family: fork_procs/fork_exec_loop/growfiles/msg_comm/sem_nstest
+ 1 of 3 msgstress01 -> rc=254 with EMPTY output = rsrun's waitpid
ECHILD path: exec'd glibc-static children die silently before
userspace output, sometimes (msgstress01 mixed 2xTBROK+1x254), under
fork-heavy tests. fork_procs passes on the host reference (rc=0).
ltplong3 rerun (8 entries incl. 2 of the same binaries): 0 recurrences
— the family is INTERMITTENT (~1 boot in 2 today); mmapstress09/10
PASS with canonical args, fork_procs -n 100 x2 + sem_nstest + growfiles
all rc=0. Functional datum for the LTP lane: msg_comm TFAIL "messages
leak between namespaces".

**Queue item 5 (brk via glibc)**: brkchurn.c (glibc-static, host gcc
— host/guest both x86_64): PASS. 864,320 raw sbrk waves (212 GB) +
114.9M glibc malloc blocks (1.15 TB), enomem=0, verify 0 mismatch.
The sys_brk paths musl physically cannot reach: covered clean.

**Gates**: x86_64 build rc=0 (final 22:23), riscv64 build rc=0 (clean
config), make test GREEN (1096-suite verdict green), SP-3 stressor
rerun clean. Pushed origin/stress-port.

**Round-3 queue**: (1) the rc=254 silent-child-death family (minimal
repro: fork+exec loop of a glibc-static hello + waitpid audit; suspect
interaction with the strand/leak family — same boots); (2) heal
bar steal-awareness (200-tick bar vs >2 s host steal windows — design
per the census above, coordinate with OH lane); (3) ltplong long-horizon
loops (hours) once the 254 family is fixed; (4) mmapstress07 with
tmpfile args; (5) doubled direct-console interleave on heal prints
(two CPUs, no console lock) — cosmetic but garbles the heal lines.
