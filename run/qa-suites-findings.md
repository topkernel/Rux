# qa-suites lane — verdict ledger (run/qa-suites-findings.md)

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
