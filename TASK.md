# arch-s1 lane: S-1 model-checking pilot (Kani/loom on kernel concurrency primitives)

## Charter (from the survey shortlist, docs/architecture/os-kernel-survey.md S-1; user go 2026-11-03)
Pilot, 1-2 sessions: evaluate whether Kani (verification) and loom
(concurrency model checking) can run against Rux kernel concurrency
primitives in a HOST-side harness, and whether they pay. This is the
methodology adoption the survey recommends (seL4-lineage verification
methods applied to Rust-native primitives).

## Known constraint going in
The kernel crate cannot link std (no_std; unit tests collide with
panic_impl — recorded house lesson). loom/kani need host builds. The
pilot's core question is EXTRACTION cost: can the primitive's logic be
built for the host (cfg-gated, or extracted to a small inner crate)
without semantic drift?

## Targets (in order)
1. kernel/src/sync/mutex.rs SleepMutex (the R84 handoff state machine:
   waiters/tokens/anti-starve — historically bug-dense, now the
   canonical primitive).
2. kernel/src/sync/spinlock.rs + the preempt-count interplay (if time).
3. The futex wait-queue key resolution (physical-word keys from the
   LTP fix) if S-1 finishes early.

## Deliverables
- A host-side harness (kani/loom configs in-repo, English comments)
  with the extraction pattern documented.
- Verdict per target: invariants checked, bugs found (loom explores
  interleavings; kani proves bounds/overflow), false-positive load,
  runtime cost, and whether the extraction changed the code shape.
- A do/don't recommendation for CI integration (GitHub smoke.yml runs
  kani on nightly already for some crates — check .github/workflows).
- NO kernel-behavior changes: extraction shims must be cfg(test)-like
  and default-off; if a real bug is FOUND, fix it through the normal
  lane discipline (separate commit, gates).

## Session log (append per session)

## Session 1 (2026-11-03, first session)

### What ran

Tooling: kani 0.68.0 (host install was a broken empty bundle — the
`cargo kani setup` download from github.com times out on this host;
fetched the release tarball via the api.github.com asset redirect
(checksum-verified) and re-created the `toolchain` symlink the setup
normally makes). loom 0.7.2 from crates.io (reachable; only github.com
itself is blocked).

Extraction (target #1, SleepMutex): `kernel/verify/src/sync/
mutex_model.rs` — the R84/R85 state transitions copied text-faithfully
with environment hooks as parameters (jiffies -> `now`, wake_up_one
outcome -> bool at the release site, ANTI_STARVE_JIFFIES -> param).
NOT a cfg-gate inside the kernel crate: the house mirror-crate pattern
(kernel/verify) already solves the no_std/panic_impl collision, so the
pilot extends it. Drift control: new `sync/mutex_model` mapping in
scripts/verify_sync_check.py (env-hook transforms + anchor-based
region comparison for the recheck/release/epoch blocks inline in
lock()/release()); `python3 scripts/verify_sync_check.py
sync/mutex_model` green. release_if_owner is mirrored but not
text-compared (unit-tested instead — its was_held wrapper does not
survive normalization).

### Target #1 verdict: SleepMutex handoff state machine

- Invariants checked: INV-M1..M4 (locked/owner/depth coupling) after
  every state section; INV-M5 token conservation (waiters == counted
  tasks [+ leaked units]); INV-M6 no underflow (kani arithmetic).
- loom (feature `loom`, RUSTFLAGS --cfg loom, LOOM_MAX_PREEMPTIONS=3,
  release): 8 scenarios — S1 two-task pure fair (clean, strand-free),
  S2 three-task (timer-rescued class), S3 barging ungated spinner
  (timer-rescued), S4 recursion+do_exit, S5 anti-starve override, and
  P1/P2/P4 planted bugs (all CAUGHT: P1 via loom deadlock detection —
  the pre-R85 b148 wedge shape; P2/P4 via INV-M5 in the spending
  section itself).
- kani: `verify_mutex_ops_invariants` (every disciplined 6-step op
  sequence keeps INV-M1..M5, no arithmetic panic) and
  `verify_mutex_exit_leak_is_bounded` — both SUCCESSFUL, 2.2 s total.
  The two counterexamples kani produced on the way were HARNESS
  call-discipline gaps (pid-0 waiter aliasing no-task ownership; a
  counted waiter also taking ungated) — now encoded as documented
  kani::assume discipline; neither is producible by a kernel lock()
  call site. False-positive load: LOW and each was decidable in
  minutes from the concrete playback.

### REAL BUG FOUND AND FIXED (kernel commit on this branch)

loom's FIRST run (90 s in) violated INV-M3 (`!locked => depth == 0`)
on the anti-starve scenario: the acquire transitions' recursion arms
tested only `owner == pid`, and a FREE lock carries owner == 0 — the
same value `get_current_pid()` returns for the no-task context
(early boot / secondary harts — the exact contexts the degrade path
serves; lock()'s adaptive fair-try runs BEFORE the can_sleep check).
A pid-0 caller hitting a momentarily-free lock behind a counted waiter
(the handoff window) "recursed into" the free lock: depth=1 on
locked=false; the bogus guard's release can then free a lock another
task genuinely holds. Fix: `st.locked &&` on the four recursion arms
(commit da166982 — behavior unchanged for every real pid; owner==pid
implies locked for pid != 0). Reachability is narrow (no-task caller
racing a contended handoff) but this is the canonical coarse lock.
Regression test pins the sequence (mutex_model_test).

Second finding (design confirmation, no code change): the epoch
recovery alone does not cover multi-waiter strands — with 3+ waiters
(sole-waiter term dead, epoch term spent) both S2-pure-fair traces and
the barger scenarios park against a FREE lock; the R85 anti-starve
timer is the designed bound (<= 5 s). The loom harness must model that
timer (park-time deadline check) and DOCUMENTS the interpretation
rule: loom deadlock + lock free + all waiters parked = bounded-latency
state, not a kernel bug. Two-task contention is strand-free without
the timer (S1 proves it exhaustively).

### Cost numbers (this host)

- Fast suite: 16 model tests, <0.1 s (1118 pre-existing tests 0.2 s).
- loom suite (8 scenarios, preemption bound 3, release build):
  59.5 s wall, all green — under the survey's 5-minute exit criterion,
  so even per-push is affordable while the suite stays this size
  (growth is exponential in threads/rounds; keep scenarios small).
- kani mutex harnesses: 2.2 s wall (0.07 s + 2.13 s).
- Kernel builds: x86_64/riscv64/aarch64 all rc=0 with the fix.
- make test (riscv64, private rootfs copy): ALL TESTS PASSED, rc=0;
  my QEMU swept by cwd match (other lanes' instances untouched).

### Code-shape drift

Kernel source: +`st.locked &&` on four arms and expanded comments —
no structural change. The extraction is a parallel copy kept honest by
the sync check (which caught nothing beyond the intentional hooks
after the transforms were written once). The LOOM DRIVER (mutex_loom.rs)
is new model code (~450 lines) — the queue/park/condvar environment;
it is NOT sync-checked (it models, not copies); its fidelity risks
found during the pilot are documented inline (pop-on-wake equivalence,
phantom-token stale flag — fixed via the dequeue_task analog, W1
refused-wake shapes out of scope).

### CI recommendation: DO (staged)

1. KEEP the kani nightly (kani.yml) — and note it was BROKEN on main
   (vma_kani missing can_merge/merge — fixed in this branch); the
   mutex harnesses add ~2 s.
2. ADD loom (new .github/workflows/loom.yml in this branch): sync-check
   + mutex scenarios, LOOM_MAX_PREEMPTIONS=3, release — 59.5 s on this
   host, so it could ride per-push; recommendation is nightly +
   workflow_dispatch first (cheaper to keep honest), promoting to
   per-push only if a sync/ regression class ever escapes it. The
   interpretation rule (timer-rescued deadlocks need a human on first
   failure) also favors nightly.
3. Rule for writers: changes to kernel/src/sync/mutex.rs must update
   kernel/verify/src/sync/mutex_model.rs in the same commit (the
   smoke-time sync check enforces it); loom scenario growth should
   stay at 2-3 threads, <=2 rounds, bound 3.
4. The survey's exit criteria: (a) planted-bug reproduction — P1/P2/P4
   all caught; (b) clean on current main — yes, after the pid-0 fix;
   (c) runtime — fast suite 0.1 s, kani 2.2 s, loom minutes (fits
   nightly, not make-test adjacency); (d) the written rule — item 3
   here plus the sync-check CI wiring.
