# qa-suites lane — verdict ledger (run/qa-suites-findings.md)

## Charter (user directive 2026-11-04, preserved from the round-1 branch tip f4e1e4f2)
"业界还有比较流行的测试用例——靠更多测试用例把内核质量搞上去。"
Onboard the industry suites we do not yet run, ranked by bug-finding
yield on OUR weak families (timers/wakeups/IPC/net-loopback/fs/IO)
and portability (static musl x86_64, serial console, KVM). TIMEOUT/
HANG/n-a results are P0 (house rule) — they outrank every ratio row.
Onboarding order v1: 1 rt-tests, 2 kselftest portable subset,
3 fio (IO engines), 4 netperf/iperf3/sockperf (loopback first),
5 Trinity (bounded), 6 syzkaller repro corpus (curated ~50),
7 bonnie++/postmark/dbench, 8 UnixBench. Rules: static musl builds via
the house pattern (tarball + build script under userspace/tests/),
one-class-per-boot on x86_64+KVM, verdicts in this file, kernel fixes
through lane discipline, cross-domain hunts hand off via the
coordinator, sources ONLY as tarball, NEVER "the L-word" in commits
(use "the reference kernel").

Per-suite, per-class verdicts per the charter: PASS / FAIL(named bug
+ minimal repro) / TIMEOUT (= P0, immediate). Kernel: origin/main
f4e1e4f2-era worktree, x86_64 bzImage, QEMU q35 + KVM, -smp 2 -m 2G,
private image copy run/rootfs-qa.img (from the permanent store),
debugfs-injected binaries, PID 1 = /test/qa/qsrun (test/qrtests/,
sources in userspace/tests/probes.tar.gz).

## Suite 1: rt-tests v2.11 (onboarded 2026-11-04)

Sources: userspace/tests/rttests/rt-tests-v2.11.tar.gz (kernel.org
cgit snapshot, tag v2.11, committed unmodified). Build:
userspace/tests/rttests/build-x86.sh (musl static hackbench +
cyclictest; host-glibc-static pi_stress — musl reports
_POSIX_THREAD_PRIO_INHERIT==-1 and upstream refuses to build without
real PI mutexes). Boot driver: test/qrtests/run-suite-x86.sh. Logs
under run/qa-runs/<class>.log.

| class | cmd | verdict | numbers / repro |
|---|---|---|---|
| hackbench socketpair/process | `hackbench 20 process` (10 groups, 400 tasks) | PASS | rc=0, Time 0.811 s |
| hackbench threads | `hackbench -T 20` | PASS | rc=0, Time 1.003 s (400 pthreads) |
| hackbench pipe | `hackbench -p 20` | PASS | rc=0, Time 0.406 s |
| hackbench scale | `hackbench -g 40 -l 500` (1600 tasks x 500 msgs) | PASS | rc=0, Time 22.978 s |
| hackbench inet 1-group | `hackbench -i -g 1 -f 4` | PASS | rc=0, Time 0.037 s |
| hackbench inet >=2 groups | `hackbench -i -g 2 -f 10` (and larger) | **FAIL(bug)** | rc=1 `connect (error: Operation in progress)` — see F-1 |
| cyclictest SCHED_OTHER | `cyclictest -D 30 -i 1000 -m` | PASS | C=29962/30000 (38 missed, 0.13%), Min 37 / Act 100 / Avg 92 / **Max 2602 us**; policy other/other |
| cyclictest SCHED_FIFO p98 | `cyclictest -D 30 -i 1000 -m -p 98` | PASS | C=29958/30000 (42 missed), Min 39 / Act 93 / Avg 95 / Max 2132 us; policy fifo, p98 accepted |
| pi_stress | `pi_stress -D 15 -g 2` | **FAIL(gap)** | rc=1 `initializing mutex: Operation not supported` — see F-2 |

No TIMEOUT/P0 hangs in this round. `pi_stress -g 4` correctly refuses
(groups > online CPUs); positional `process|thread` args are not in
v2.11 (use `-T`).

## Named findings

### F-1 (kernel net stack): loopback non-blocking TCP connect never completes inline — EINPROGRESS surfaced
- **Repro**: `hackbench -i -g 2 -f 10` (run/qa-runs/inet-sweep.log):
  each inet pair does socket/bind/listen, FIONBIO on both ends,
  `connect(s2, 127.0.0.1)` — returns EINPROGRESS and the connection
  never becomes usable; hackbench barfs. 1 group x 4 pairs passes;
  every larger shape fails identically. At the default 10 groups a
  secondary EMFILE follows.
- **Reference semantics**: the same binary pattern works on the
  reference kernel (hackbench -i is exercised upstream): loopback
  non-blocking connect completes synchronously and connect() returns 0.
- **Owner**: lmbench-fix / OH-net (the P0 loopback-TCP hunt) — handoff
  via coordinator.

### F-2 (known gap): PI futex family returns ENOSYS
- kernel/src/sync/futex.rs `do_futex` `_ => -ENOSYS` ("PI-related
  operations not yet supported"): FUTEX_LOCK_PI/UNLOCK_PI/TRYLOCK_PI/
  LOCK_PI2 and WAIT/CMP_REQUEUE_PI. glibc PI pthread mutexes therefore
  cannot initialize (pi_stress rc=1).
- **Minimal repro**: `syscall(SYS_futex, &m, FUTEX_LOCK_PI, 1, 0, 0, 0)`
  → -1 ENOSYS (m = tid-word).
- **Owner**: sched/sync domain — handoff via coordinator (stress/sched
  lane + LTP futex families).

### F-3 (toolchain, SHARED asset): x86_64 musl libc.a ships ENOSYS stubs
- toolchain/x86_64-rux-linux-musl/lib/libc.a contains 25-byte stub
  objects unconditionally returning -ENOSYS for:
  sched_getparam, sched_getscheduler, sched_setparam,
  sched_setscheduler, **utimensat**. The kernel implements all five
  (raw syscalls succeed — verified by test/qrtests/schedprobe.c,
  run/qa-runs/schedprobe.log).
- **Impact**: every musl-static guest binary using these wrappers
  silently gets ENOSYS (this is what killed cyclictest at
  "unable to get scheduler parameters" before the workaround).
- **Workaround (this lane)**: userspace/tests/rttests/x86-sched-wrappers.c
  linked ahead of libc.a (strong defs override archive members).
- **Action**: toolchain tarball needs a rebuild without the stubs —
  coordinator to route to the toolchain owner. Same audit of the
  riscv64 toolchain is advisable.

### Observations (not new, cross-domain)
- `INFO: task 306:306 blocked for more than 120/179 s` hung-task
  report fires on EVERY boot after the suite finishes; pid 306
  deterministic, kernel stack parks in context_switch after
  fork-child handling. Same family already present in stress-port
  logs (k10 series) — owner: stress/sched hunt, not re-reported.
- Serial console echo garbling when userspace printf interleaves with
  dfx taskdump-walk prints (e.g. `pi_stresPs A-DR 1K5 W-gALK- 4AR`).
  RES/verdict lines unaffected. Owner: dfx/console family.
- Default RLIMIT_NOFILE soft 1024 = reference parity (the 10-group
  inet shape legitimately exceeds it after F-1 is fixed; raise with
  ulimit in-shell or smaller -f).

## Round 1 status
- rt-tests: ONBOARDED (8 classes run, 6 PASS, 2 FAIL with named
  owners). Next: kselftest portable subset, then fio.

## Round 2 — full battery rerun on main 0c8e4b79 (rebase; kernel net/
sched/timer code unchanged vs round 1 — only a default-off dfx print
probe in file.rs landed between). Rig identical, logs in
run/qa-runs-r2/ (fresh-boot classes, one per boot).

| class | round 1 (d08f8cd7) | round 2 (0c8e4b79) | verdict |
|---|---|---|---|
| hackbench socketpair `20 process` | PASS 0.811 s | PASS 0.791 s | PASS (steady) |
| hackbench pipe `-p 20` | PASS 0.406 s | PASS 0.415 s | PASS (steady) |
| hackbench threads `-T 20` | PASS 1.003 s | PASS 0.807 s | PASS (steady) |
| hackbench scale `-g 40 -l 500` | PASS 22.978 s | PASS 22.303 s | PASS (steady) |
| hackbench inet `-g 2 -f 10` | FAIL F-1 (EINPROGRESS, rc=1, T=0) | **TIMEOUT** — 40 tasks block >120 s until qsrun TERM (`TIMEOUT-TERM`, run/qa-runs-r2/hackbench-inet-g2.log) | **TIMEOUT = P0-class** (F-1 variant — see below) |
| cyclictest SCHED_OTHER `-D 30 -i 1000 -m` | PASS 38 missed, avg 92, max 2602 | PASS 38 missed, avg 95, max 3149 | PASS (same class; timers domain steady) |
| cyclictest SCHED_FIFO p98 | PASS 42 missed, avg 95, max 2132 | PASS 33 missed, avg 89, max 1142 | PASS (same class) |
| pi_stress `-D 15 -g 2` | FAIL F-2 ENOSYS | FAIL F-2 ENOSYS (identical) | FAIL(gap), unchanged |

### F-1 update (round 2): symptom is BOOT-STATE dependent — fresh boot HANGS instead of erroring
- Round 1 ran inet classes sequentially in ONE boot: g=1 completed
  first, then every larger shape failed instantly with
  `connect (error: Operation in progress)` rc=1.
- Round 2 ran `hackbench -i -g 2 -f 10` on a FRESH boot as the first
  class: NO EINPROGRESS — all 20 server listens happen (tcp-listen
  ports 32768..32810), every client handshake completes (SYN/SYN+ACK/
  ACK traced), then only ONE pair ever carries data (12x repeated
  PSH+ACK/ACK on 32769->32768, retransmit-shaped), the other 39 tasks
  never make progress, and the boot sits blocked until the 120 s
  qsrun TERM. hackbench's TERM handler longjmps out and exits rc=0 —
  the RES rc=0 is a FALSE PASS; the `TIMEOUT-TERM` marker is the real
  verdict (P0 house rule).
- Conclusion for the owning hunt: the loopback TCP defect is not
  (only) in connect — on a cold stack the handshake path works and the
  DATA path stalls (single-pair retransmit loop); connect() inline
  completion (EINPROGRESS surfacing) depends on warm boot state.
- Repro (deterministic, 2 boots): fresh boot `hackbench -i -g 2 -f
  10` → hang; same boot after `hackbench -i -g 1 -f 4` → instant
  EINPROGRESS rc=1. Kernel net code identical in both rounds.
- Owner unchanged: lmbench-fix / OH-net P0 loopback-TCP hunt —
  handoff updated via coordinator (round 2).

### Harness note (qsrun)
- TIMEOUT-TERM classes must be read from the marker, not the RES rc:
  upstream hackbench's SIGTERM handler longjmps and yields rc=0.

## Round 2 status
- rt-tests battery on main 0c8e4b79: 5 PASS, 1 TIMEOUT (P0-class
  handoff, F-1 variant), 1 FAIL(gap) F-2, plus 1 FAIL-toolchain F-3
  unchanged. Timers-domain probe (cyclictest) steady on new main.
- Next: kselftest portable subset.

## Round 3 — battery on new main 66cd9918 (LTP async-close + OH CSUM_V3
## + stress rlimits/spin/wake_up landed between) + this lane's fixes

Kernel revs (all in this round's boots, marked per row):
- K1 = 66cd9918 + memfd reference rework + vm.memfd_noexec/pidns scope
  + pidns tgkill/tkill + TSC ratchet CAS (through bca03e1b)
- K2 = K1 + CLOCK_BOOTTIME/CLOCK_TAI in nanosleep/POSIX timers
  (255f86f5) — only the three timer-clock rows + the memfd probe.

### rt-tests battery (round-3 item 1) — one fresh boot per class, K1

| class | round 2 (0c8e4b79) | round 3 (K1) | verdict |
|---|---|---|---|
| hackbench socketpair `20 process` | PASS 0.791 s | PASS 0.858 s | PASS (steady) |
| hackbench pipe `-p 20` | PASS 0.415 s | PASS 0.415 s | PASS (steady) |
| hackbench threads `-T 20` | PASS 0.807 s | PASS 0.890 s | PASS (steady) |
| hackbench scale `-g 40 -l 500` | PASS 22.303 s | PASS 22.194 s | PASS (steady) |
| hackbench inet `-i -g 2 -f 10` | TIMEOUT-TERM | **TIMEOUT-TERM** (120 s, 22 listens, same stall) | **F-1 UNCHANGED on new main** — P0 handoff stands (net lane) |
| cyclictest SCHED_OTHER | 38 missed, avg 95, max 3149 | **25 missed, avg 89, max 1289** | PASS (**improved**: max -59 %, misses -34 %) |
| cyclictest SCHED_FIFO p98 | 33 missed, avg 89, max 1142 | **2 missed, avg 87, max 396** | PASS (**improved**: max -65 %, misses 33x→2) |
| pi_stress `-D 15 -g 2` | FAIL F-2 ENOSYS | FAIL F-2 ENOSYS (identical) | FAIL(gap), unchanged |

- The cyclictest max-latency rows moved substantially on the new main
  (stress lane's wake_up/optimistic-spin/rlimits fixes are the likely
  producers; sched domain — worth a pbench P2 context-switch row when
  the coordinator gates this merge).
- F-1 (loopback TCP): 3rd fresh-boot reproduction (r2, r3 pre-battery,
  r3 battery) — deterministic. Owner unchanged: net lane P0 hunt.

### Round-3 kernel fixes (this lane, all reference-parity, x3 + make test green)

1. **memfd reference rework** (round-2's dying work, completed + driven
   to a verdict): seal state model, F_SEAL_EXEC, ftruncate/write/mmap/
   mprotect/fallocate/chmod rules, /proc/pid/fd reopen, 0777 mode
   matrix — memfd_test runs CREATE..SEAL-RESIZE green.
2. **vm.memfd_noexec** (per-pidns, chain-max read, CAP_SYS_ADMIN +
   floor-guarded write) + memfd_create scope matrix (0/1/2).
3. **pidns tgkill/tkill**: caller-ns tid resolution (was global —
   abort() inside CLONE_NEWPID died SIGSEGV because raise() could not
   deliver SIGABRT). Reference do_tgkill uses find_vpid; ours now too.
4. **TSC ratchet CAS**: threadtest caught CLOCK_MONOTONIC stepping
   backward a few hundred ns — the ratchet published with a plain
   store; two CPUs could interleave a clobber. compare_exchange_weak
   publish now.
5. **CLOCK_BOOTTIME/CLOCK_TAI in nanosleep + POSIX timers**: set-timer-
   lat rc=1 was timer_create(BOOTTIME/TAI)=EINVAL. BOOTTIME is
   monotonic-domain (no suspend); TAI = realtime + constant 37 s.

### kselftest subset verdicts (round-3 item 2)

| class | round 2 (0c8e4b79, budget-starved) | round 3 | verdict |
|---|---|---|---|
| memfd | rc=134 at sysctl (no sysctl node) + newpid SIGSEGV | rc=134 — last FLUSHED print is SEAL-FUTURE-WRITE, but the fault is in the nested-sysctl child (see F-4 decode) | **FAIL(bug F-4)** — all seal legs green |
| nanosleep | pass:2 skip:10 (T=4 s) | K2: TAI/BOOTTIME now sleepable | see below (K2 rows) |
| nsleep-lat | rc=0, BOOTTIME/TAI unsupported | K2 rerun | see below |
| set-timer-lat | rc=1 (BOOTTIME/TAI timer_create EINVAL) | K2 rerun | see below |
| mqueue-lat | OK (T=10 s) | OK (T=10 s) | PASS |
| inconsistency-check | TIMEOUT (60 s TERM mid-run — 10 s/clock x 12 needs 120 s+) | **rc=0, 12/12 ok** (`-t 5`) | PASS (was under-budgeted) |
| raw_skew | TIMEOUT at 90 s (test sleeps 120 s by design) | **rc=0, drift 0.0 [OK]** (180 s budget) | PASS (was under-budgeted) |
| threadtest | [FAILED] — CLOCK_MONOTONIC backward steps | no [FAILED] in-window but the boot overran the QEMU budget; quiet rerun (pass2): test output NEVER appeared, boot wedged — see F-5 | **FAIL(TIMEOUT)** — sched-domain handoff (F-5) |
| posix_timers | TIMEOUT at 60 s (itimer prof "Diff too high: 3" + signal-distribution stall) | TIMEOUT at 180 s + qsrun wedged in the TERM teardown (KSIG9/KOT-KILL cascade) | **FAIL(TIMEOUT)** — timers-domain handoff |

### K2 rows (255f86f5-class kernel, pass2 fresh boots) — the three
### CLOCK_BOOTTIME/CLOCK_TAI verdicts r3 was mid-writing

| class | round 2 (0c8e4b79) | K2 (255f86f5) | verdict |
|---|---|---|---|
| nanosleep | pass:2 skip:10, BOOTTIME/TAI EINVAL | rc=0, **pass:4** skip:8 — ok 8 CLOCK_BOOTTIME, ok 12 CLOCK_TAI (T=8 s) | **PASS** (fix verified; harvested to main as ad0d5cb4) |
| nsleep-lat | rc=0, BOOTTIME/TAI [UNSUPPORTED] | rc=0 — CLOCK_BOOTTIME [OK], CLOCK_TAI [OK] (T=85 s) | **PASS** (fix verified) |
| set-timer-lat | rc=1 — timer_create(BOOTTIME/TAI)=EINVAL | rc=0 — all [OK] incl. TAI one-shot max latency 135.3 us (T=88 s) | **PASS** (fix verified) |
| threadtest quiet rerun | — | `DFX=" "` boot: no test output at all; 8 workers spawn, pid 306 (qsrun forker) blocks >120 s, TERM at 300 s | **FAIL(TIMEOUT)** — F-5 |

## Round-3 named findings (completing the mid-write)

### F-4 (kernel pidns/mm): kselftest memfd_test dies in the NESTED-sysctl child, NOT at SEAL-FUTURE-WRITE
- The round-3 table's "rc=134 at SEAL-FUTURE-WRITE" attribution was a
  stdout-buffering artifact: on the serial (non-tty) console the stock
  binary's stdout is fully buffered; everything after SEAL-FUTURE-WRITE
  (SEAL-SHRINK/GROW/RESIZE, sysctl tests) stayed in the 8 KB buffer.
- Kernel-side decode (run/qa-runs-r3/memfd.log):
  - sysctl_simple_child (1st CLONE_NEWPID spawn, pid 309) exits CLEAN
    (PIDNS-KILL ns-init victims=0) — the simple sysctl 0/1/0/2/1/0
    sequence + vm.memfd_noexec write/memfd_create matrix all work
    (kernel prints "memfd_create() requires MFD_NOEXEC_SEAL with
    vm.memfd_noexec=2" on the expected-EPERM probe).
  - sysctl_nested_child (2nd spawn, pid 310) takes `trap: #GP at
    rip=0x403b63, mode=user` → SIGDEATH sig=11. 0x403b63 symbolizes
    into musl abort() (0x403ae2): its terminal `hlt` at CPL3 is #GP.
    So the child called abort() — a join_thread/sysctl assertion —
    and died inside abort's crash path; SIGABRT/self-kill prints
    downstream (SIGDEATH pid=307 sig=6, rc=134) are the parent's
    abort on the signaled child.
  - Its own nested child (pid 311) was then PIDNS-KILLed as a
    consequence (ns-init death cascade) — not an independent fault.
- memfdstep.c (step-isolated test_seal_future_write incl.
  fork-private-write COW legs) passes ALL steps on the same kernel —
  consistent: the seal legs are fine; the abort is in the nested
  pidns sysctl legs (nested CLONE_NEWPID children + SIGSTOP/SIGCONT
  choreography + sysctl read/write after fork).
- Round-4 decoder: memfd-unbuf (same memfd_test.c, per-call-flushed
  printf via the userspace/tests/kselftest/memfd-unbuf.c shim) — the
  last printed line names the exact abort leg. Result below (r4).
- Owner: this lane (the memfd_noexec/pidns surface we landed in r3)
  unless the decode points elsewhere.

### F-5 (kernel sched, cross-domain): threadtest wedges the whole boot — test output never appears
- Round 2: [FAILED] CLOCK_MONOTONIC backward steps → root-caused and
  fixed by the TSC ratchet CAS (bca03e1b) — the ratchet published with
  a plain store; two CPUs could interleave a clobber.
- Round 3 battery (K1, dfx=taskdump,watchdog): no [FAILED] line, but
  the boot overran the QEMU budget.
- Round 3 quiet rerun (pass2, `DFX=" "` so no dfx prints): threadtest
  printed NOTHING beyond its /etc/localtime open; its 8 worker threads
  spawn, then the serial fills with UNCONDITIONAL WAKE-HEAL-SKIP /
  STALE-CLAIM-RESET / SCHED-IRQOFF prints (sched-lane instruments now
  on main — rate-limited to 60/site but NOT dfx-gated), pid 306
  (qsrun forker) blocks >120 s (same HUNG-CTX family as the
  after-suite observation in round 1), TERM at 300 s, no RES line.
- Two handoffs, one to each owner (via coordinator):
  1. the threadtest wedge itself — sched domain (wake-path /
     stale-claim neighborhood; the parked-forker shape matches the
     stress-port k10 family);
  2. the unconditional sched console instrumentation on main — it
     fires on EVERY loaded boot and is a timing-hygiene liability for
     latency rows (cyclictest medians improved anyway in r3, so it is
     not currently dominating, but it should be dfx-gated or moved to
     tracepoints per the 2026-11-02 observability policy).

### posix_timers TIMEOUT (timers-domain handoff, unchanged from r3 table)
- rc absent at both 60 s and 180 s budgets; the run ends in the
  KSIG9/KOT-KILL teardown cascade (qsrun TERM → posix_timers children
  signal each other, group kills loop). r2 shape: itimer prof
  "Diff too high: 3" + a signal-distribution stall mid-test. P0-class
  per house rules; owner: timers domain.

### Arch-file edits clarification (for the coordinator's r2 revert question)
The only kernel/src/arch/ edits this lane has ever carried are three
lines, one per arch:
`pub const EPERM: i64 = -1;` added to the `mmap_error` tables in
kernel/src/arch/{x86_64,riscv64,aarch64}/mm/memory_layout.rs.
- Where they came from: the memfd reference-parity commit (originally
  45e8afea in round 2, rebased to e3f0dc02 in round 3) — identical
  content in both; the commit message documents them ("mmap/mprotect
  writable MAP_SHARED under F_SEAL_WRITE/F_SEAL_FUTURE_WRITE is EPERM
  (mmap_error::EPERM added on all three arches)").
- Why they exist: the reference kernel's shmem_mmap returns -EPERM for
  writable shared mappings of a sealed memfd; Rux's mmap errno
  constants live in the per-arch mmap_error tables (alongside
  EBADF/EOPNOTSUPP/EEXIST) and EPERM was missing. No behavioral arch
  change — a constant the fs seal path returns.
- Why the coordinator saw them as unexplained: they ride in the memfd
  commit, which is NOT yet harvested to main; when harvesting only
  255f86f5 (timers) as ad0d5cb4, the branch's arch delta had no
  visible owner in the timers diff. Nothing to reintroduce — they are
  already in e3f0dc02 awaiting harvest with this note as the intent.

## Round 4 — battery on merged main 8d47783a + lane fixes; F-4 CLOSED

Kernel: merge b6f49579 (main 8d47783a: jbd2 csum, sched wake-contract
fix, CLOCK_BOOTTIME harvest, tracepoints) + r3 lane commits. Fresh boot
per class, logs run/qa-runs-r4/.

### rt-tests battery (round-4 item 1)

| class | round 3 (K1) | round 4 (merged) | verdict |
|---|---|---|---|
| hackbench socketpair `20 process` | PASS 0.858 s | PASS 0.815 s | PASS (steady; 4 rounds within 0.79-0.86) |
| hackbench pipe `-p 20` | PASS 0.415 s | PASS 0.420 s | PASS (steady; 4 rounds within 0.41-0.42) |
| cyclictest SCHED_OTHER `-D 30 -i 1000 -m` | 25 missed, avg 89, max 1289 | **26 missed, avg 89, max 1018** | PASS (max-latency trend improves every round: 3149 r2 → 1289 r3 → 1018 r4) |
| cyclictest SCHED_FIFO p98 | 2 missed, avg 87, max 396 | **28 missed, avg 89, max 1857** | PASS but see note (isolation rerun below) |
| pi_stress `-D 15 -g 2` | FAIL F-2 ENOSYS | FAIL F-2 ENOSYS (identical) | FAIL(gap), unchanged — PI-futex owner handoff stands |

- cyclictest FIFO p98 note: r4's one boot shows 28 missed / max 1857 us
  vs r3's 2 / 396. Misses remain <0.1 % of 30k cycles and the avg is
  unchanged (87-89); the max moved. Main gained the pickgen
  LP-WAKE-OUTCOME instruments + wake-contract fix between r3 and r4 —
  sched domain should see the isolation rerun row (below) and, if the
  class matters at gate time, a pbench P2 row.
- Both cyclictest rows now run on the CLOCK_BOOTTIME/TAI-fixed timer
  code (r3 rows predate 255f86f5) — clean numbers as requested.

### kselftest subset (round 4)

| class | round 3 | round 4 | verdict |
|---|---|---|---|
| memfd | FAIL F-4 (rc=134, nested-sysctl child) | **rc=0, `memfd: DONE`** — every leg green | **PASS — F-4 FIXED** (3 fixes below) |
| threadtest | FAIL(TIMEOUT) F-5 | TIMEOUT again (no RES; qsrun forker blocks >120 s) | **FAIL(TIMEOUT) — F-5 stands**, sched handoff |
| posix_timers | FAIL(TIMEOUT) | TIMEOUT again (no RES, no QS-ALL-DONE) | **FAIL(TIMEOUT) — timers handoff stands** |

### F-4 root causes and fixes (this lane; all reference-parity, verified by probe + full test)

1. **pidns depth-2 numbering (kernel/src/process/ns.rs, task.rs)**:
   a CLONE_NEWPID child of a task that is ITSELF in a pidns got clone(2)
   returning its GLOBAL pid, but waitpid/kill resolve by caller-ns
   numbers → the nested-sysctl child aborted with waitpid ECHILD on its
   own just-spawned child. Fix: per-level upid chain on Task (the
   reference struct pid's upid[] semantics): copy_namespaces allocates
   the child a fresh number in every intermediate ns; all six
   translation/resolution helpers (translate_sender_pid_ns,
   resolve_vpid/vtid, resolve_local_pid_to_global, wait_vpid, task_vpid)
   resolve through the chain; init-ns view stays the global pid.
2. **SIGSTOP/SIGCONT on a pid-ns init (kernel/src/signal.rs)**: the
   delivery-path SIGNAL_UNKILLABLE gate discarded every non-handled
   signal for ns inits — SIGSTOP included, so kselftest's
   sysctl_nested_wait children never parked (they ran ahead of the
   parent's sysctl write, then "write sysctl 0 succeeded, but failure
   expected"). Reference get_signal() applies SIGNAL_UNKILLABLE only to
   FATAL default actions; group-stop signals stop an ns init and SIGCONT
   resumes it. Gate now passes 18-22 through.
3. **F_ADD_SEALS on an O_RDONLY fd (kernel/src/fs/memfd.rs)**: reference
   memfd_add_seals refuses with EPERM unless the fd is open for writing
   (test_share_open's O_RDONLY reopen). Gate added.

Decode trail: memfd-unbuf (per-call-flushed memfd_test via the
memfd-unbuf.c shim) named the abort leg ("newpid thread: waitpid()
failed: No child process"); memfdsysctl.c (step probe replicating the
"0 -> 1 after fork" leg) isolated the SIGSTOP non-park; then the two
gates. memfd: CREATE through DONE green, including all nested-sysctl
"after fork" legs and both SHARE (plain + shared file-table) batteries.

### Tooling (round 4)
- userspace/tests/kselftest/unbuf.c: generic unbuffered-printf shim
  (-DUNBUF_TARGET='"<test>.c"'); posix_timers-unbuf + threadtest-unbuf
  built alongside the stock binaries (stall-site decoders).
- run/r4-battery.sh, run/r4-pass2.sh, run/r4-pass3.sh: the round's
  boot scripts.

### Round-4 pass-3 (final kernel with the three F-4 fixes) + gates
- memfd (STOCK binary, official class boot): **rc=0** — F-4 fix
  confirmed on the unmodified test.
- cyclictest FIFO p98 isolation rerun: 27 missed, avg 88, max 1352
  (two r4 boots: 28/1857, 27/1352 vs r3 single boot 2/396; avg stable
  87-89 across all rounds). Sched domain should adjudicate the
  max-latency shift (main gained pickgen LP-WAKE-OUTCOME instruments
  and the wake-contract fix between r3 and r4).
- cyclictest SCHED_OTHER on the final kernel: 29 missed, avg 88,
  max 667 (best max yet) — the OTHER row is not affected by the shift.
- hackbench socketpair on the final kernel: 0.793 s (steady; the
  pidns/fork-path changes are behavior-neutral for plain processes).
- posix_timers-unbuf: NO test output even with per-call flushing —
  the wedge precedes the test's first print (threads spawn: pids
  308-310; no hung-task report this boot; qsrun's TERM teardown wedges
  after the 150 s budget). Timers-domain P0 handoff stands, sharpened:
  the stall is before any TAP/result line, i.e. in early setup or the
  first timed wait, not mid-matrix.
- threadtest-unbuf: same shape — no output at all (F-5 sharpened the
  same way: the wedge is before "using independent threads", i.e.
  during thread spawn/setup, not the measurement loop).
- Gates: PLATFORM=x86_64 make build rc=0; PLATFORM=riscv64 rc=0;
  PLATFORM=aarch64 rc=0; `make test` ALL GREEN (1096/1096) on riscv64.
