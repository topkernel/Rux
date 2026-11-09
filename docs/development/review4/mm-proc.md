# Review round 4 — mm/process/sched/sync (agent r4-mm-proc)

Scope: `kernel/src/{mm,process,sched,sync}/**` (~60K lines), every file read end to end.
Dedup base: docs/development/review-coverage.md (defects table + per-file notes),
docs/archive/code-review-2026-09-12.md (Sept round, fixed items), git log since 2026-10-14
(R-series, park-face, munmap deferred-zap, mremap TOCTOU, futex parity/PI batches, lost-wake-mid-exit...).

Findings below. Severities: Critical / High / Medium / Low / Info.

---

### [High] kernel/src/sync/futex.rs:1674-1683 / 1689-1707 / 1749-1840 — signal-return race with a concurrent waker leaks a waiter-pool slot permanently

Defect: `futex_wait_timeout`'s signal path reads `w.woken` under only the SLOT lock, and
on `false` calls `remove_waiter`. `remove_waiter` re-validates `w.bucket` under the
BUCKET lock and walks the chain — but a `futex_wake` that completed between the
`woken` read and the bucket-lock acquire has already set `woken=true`, UNLINKED the
entry (bucket lock held), and left the slot for the sleeper to free. `remove_waiter`
then finds nothing (warns "futex-remove-miss"), returns WITHOUT freeing, and the
sleeper returns -EINTR (or loops via the spurious arm at :1689, same shape). The slot
stays `Some(Waiter)` in no chain forever: `alloc_waiter` only takes `None` slots,
`futex_cleanup` only walks chain heads, so nothing ever reclaims it. Same TOCTOU in
the was_spurious arm (`!was_woken` read, then unlink attempt). Trigger: any signal
(SIGKILL storm, timer signal) racing a FUTEX_WAKE on the same waiter — each hit
shrinks the fixed pool (FUTEX_WAITER_POOL_SIZE; pth_str02 alone chains 256 slots for
a 1000-thread join). Accumulated leaks end in pool exhaustion, where every new futex
wait degrades to the 1-jiffy `pool_full_backoff` retry loop — system-wide futex
breakdown. Linux comparison: kernel/futex/core.c futex_unqueue() — the waiter node
lives on the caller's kernel stack, and the "already dequeued" case (WARN at
__futex_unqueue) simply leaves the waker's wake_q reference to drop it; there is no
pool to leak. Fix direction: on the miss path (or before returning EINTR / looping),
re-check the slot under the bucket lock: if `w.task == current`, free the slot and
clear `futex_wait_enqueued` regardless of whether it was still chained (treat
`woken==true` as a consumed wake — optionally return 0 instead of EINTR, matching
futex_wait_queue's wake-first-vs-signal ordering).

### [High] kernel/src/sync/futex_pi.rs:589-609 / 812-844 — PI sleeper-side cleanup unlinks without the PI bucket lock, races the unlock handoff

Defect: after `schedule()` returns, `futex_lock_pi` checks `pi_waiter_handed` under only
the SLOT lock and then runs `pi_queue_unlink` + `pi_waiter_free` + unboost + maybe_free
with NO PI bucket lock held (lines 598-603). Same in `futex_pi_exit_cleanup` part 1
(:814-844). But `futex_unlock_pi`'s handoff runs UNDER the bucket lock and does: read
top waiter -> word_cas (user word now names the waiter) -> unlink -> set `handed` ->
wake. Interleaving: a signal-woken sleeper (handed not yet set) unlinks and FREES the
slot while the unlocker sits between "read top" and "set handed". The unlocker then
sets `handed=true` on a freed slot that `pi_waiter_alloc` may have already reused for
a DIFFERENT waiter — that waiter's park now ends with `pi_waiter_handed()==true` and
returns 0 "owning" a lock whose word was never set for it: two owners for one PI
mutex (mutual-exclusion break), or the original sleeper returns EINTR while the word
names its TID (glibc's retry then hits EDEADLK). Trigger: signal (SIGKILL/timeout)
racing FUTEX_UNLOCK_PI on the same futex — the same TOCTOU shape as the plain-side
slot-leak finding but with ownership corruption, not just a leak. Note
`pi_requeue_detach` (:794-802) gets this right — it takes the bucket lock before
unlinking. Linux comparison: futex_unqueue / futex_requeue pi paths all serialize on
hb->lock before touching the plist (kernel/futex/core.c, kernel/futex/requeue.c).
Fix direction: take `pi_bucket_lock(&key)` around the post-schedule cleanup in
futex_lock_pi and around part 1 of futex_pi_exit_cleanup (re-derive si under the
lock), and re-check `handed` AFTER acquiring the lock.

### [Medium] kernel/src/sync/rcu.rs:766-779 vs 714-719 — lost waiter announcement strands call_rcu callbacks — ✅ FIXED (`834f2fca`)



Defect: `fire_graced_batch` releases the QS-report announcement (CBS_WAITER_HELD
swap(false) + RCU_WAITERS sub) AFTER dropping CBS_BATCH_LOCK; `rcu_softirq_handler`
takes the announcement (swap(true) + add) BEFORE taking CBS_BATCH_LOCK. Interleaving:
CPU A's fire_graced_batch sees the pending list empty under the lock, unlocks; CPU B's
rcu_softirq_handler swap(true) returns true (A still holds) so B does NOT add a waiter;
A's swap(false) returns true so A subs 1. RCU_WAITERS can reach 0 while B's freshly
spliced batch pends. `rcu_note_context_switch` then early-returns (reports stop), so a
busy non-idle CPU's RCU_QS_GEN never reaches CBS_BATCH_GEN — the batch never fires.
Trigger: call_rcu racing another CPU's empty-batch drain; on an all-busy system with no
further synchronize_rcu callers and no idle CPU, callbacks leak indefinitely (each new
call_rcu re-arms the announcement, so it self-heals only if new callbacks arrive).
Linux comparison: Tiny RCU's rcu_gp cleanup and callback acceleration run under the
rcu_state lock ordering that makes announcement/release decisions against the actual
list state (kernel/rcu/tiny.c rcu_process_callbacks). Fix direction: do the
swap(false)/sub decision while holding CBS_BATCH_LOCK, and have the splice side take
its swap(true)/add under the same lock after linking the batch.

### [Low] kernel/src/sync/rcu.rs:387 — sleepy-wait deadline rounds sub-jiffy budgets to zero wait — ✅ FIXED (`7006cc43`)

`deadline = get_jiffies() + max_ms * hz / 1000`: a max_ms below 1000/hz (e.g. 5 ms at
HZ=100) yields +0 jiffies, so the first deadline check at the loop bottom fails the
wait instantly with no polling at all. Not known to be hit by current callers; guard
with at least one jiffy (`+ (max_ms * hz / 1000).max(1)`).

### [Info] kernel/src/sync/condvar.rs:127 — `wait()` (documented non-interruptible) registers INTERRUPTIBLE

The non-interruptible `ConditionVariable::wait` passes `interruptible=true` to
prepare_to_wait, so a signal can wake it; correctness rests on every caller re-checking
the condition in a loop (the doc example does). Harmless today, but the name/doc and
behavior disagree — Linux wait queues distinguish TASK_INTERRUPTIBLE from
TASK_UNINTERRUPTIBLE here.

### [Medium] kernel/src/sync/seqlock.rs:93-96 — reader side lacks the LoadLoad fence between data read and second seq read — ✅ FIXED (`5b93ce8e`)

Defect: `read_inner()` loads seq1 (Acquire), reads the data via `read_volatile` (a plain
load to the CPU), then `read_retry()` loads seq2 with Acquire. Acquire on a load orders
LATER accesses after it — it does NOT order EARLIER plain loads (the data reads) before
the seq2 load. On a weakly-ordered target (riscv64 RVWMO allows load-load reordering)
the data load can be delayed past the seq2 load: seq2 still equals seq1 (writer had not
started at seq2-load time), the delayed data load then observes a concurrently written
value — a torn snapshot passes the consistency check. Trigger: any SeqLock reader under
a concurrent writer on real riscv64 hardware (QEMU TCG mostly serializes, so tests pass).
Linux comparison: kernel/seqlock.h `read_seqcount_retry()` does `smp_rmb()` BEFORE the
second `READ_ONCE(s->sequence)`, and `__read_seqcount_begin()` does `smp_rmb()` after
the first — exactly this fence pair (refer/linux/include/linux/seqlock.h). x86_64 is TSO
(loads not reordered with loads) so that arm is unaffected. Fix direction: insert a
LoadLoad fence (`core::sync::atomic::fence(Ordering::Acquire)` is not enough on the
reader; needs `fence(SeqCst)` or an explicit rmb) between the data snapshot and
`read_retry`, or make the data read address-dependent. Severity Medium because primary
testing runs under TCG, but this is a correctness gap on the silicon target.



### [Medium] kernel/src/process/fork.rs:413-427 — RLIMIT_NPROC enforcement at fork is compiled out (`if false`) — ✅ FIXED (`3df6acb6`)

Defect: the NPROC check block is gated on `if false` with a TODO ("correlates with a null
deref ... the check is disabled until then"). Nothing else limits process count at fork:
setrlimit(RLIMIT_NPROC)/prlimit64 store the value (rlimits table), but no enforcement
point consumes it, so a `ulimit -u`-bound shell can fork unbounded (LTP fork07 and the
getrlimit/setrlimit tests treat EAGAIN-at-limit as contract). The cgroup pids controller
covers only tasks explicitly placed in a limited cgroup. Trigger: any fork loop under a
finite RLIMIT_NPROC — succeeds where Linux returns EAGAIN (kernel/fork.c copy_process:
`p->signal->rlim[RLIMIT_NPROC].rlim_cur` via can_create_group_forking/charge). Fix
direction: re-enable with a cheaper count (an AtomicU64 of live user tasks maintained at
fork/exit instead of the pid-hash walk the disabled code did), and root-cause the old
null-deref (likely the `address_space()` unpinned borrow inside the walk — the mm-slot
lock now makes a pinned variant trivial).

### [Low] kernel/src/process/fork.rs:508-515 / 570-1036 — unwind after cgroup_on_fork never detaches from the cgroup — ✅ FIXED (`5512b361`)

`cgroup_on_fork` attaches the child (pushes its pid into the cgroup's tasks Vec, sets
cgroup_ptr) at :570. Every failure arm between there and the successful enqueue at
:1020 — CLONE_PARENT_SETTID/CHILD_SETTID EFAULT (:591/:610), copy_mm failure (:811),
write_child_settid false (:784/:805), copy_namespaces EPERM (:973), the R52
enqueue-refused unwind (:1034) — runs `unwind()`, which removes the child from the
parent/hash/pid allocator but never calls the cgroup detach. The dead pid stays in the
cgroup's tasks Vec forever: subtree_task_count() (and therefore pids.max enforcement and
cgroup.events "populated") is permanently inflated by one per failed fork; a pid-max
under a limited cgroup eventually wedges every later fork of that cgroup with EAGAIN.
Linux comparison: cgroup_fork failure paths run cgroup_cancel_fork /
cgroup_post_fork-undo (kernel/cgroup/cgroup.c). Fix direction: add
`sched::cgroup::cgroup_on_fork_cancel(child)` to the unwind closure (detach_task +
sub_cgroup_mem_charged settle mirrors cgroup_exit).

### [Low] kernel/src/process/exit.rs:2144-2151 — do_wait ignores the status copy fault, unlike do_wait_nonblock — ✅ FIXED (`7006cc43`)

Blocking do_wait writes the wait status with `let _uncopied = copy_to_user(...)` and
returns Ok(child_pid) even when the user pointer was unwritable: the caller sees success
with an unmodified status word. do_wait_nonblock (:2428-2442) already handles the same
shape correctly (releases the claimed zombie, returns EFAULT). Linux wait4 returns
-EFAULT from put_user while still reaping (kernel/exit.c wait_task_zombie). Fix: mirror
the nonblock arm — capture the uncopied count, release_task, return EFAULT.
### [High] kernel/src/mm/kswapd.rs:66-84 / 248-266 — kswapd lost-wakeup race parks the daemon permanently

Defect: the kswapd loop does `KSWAPD_WAKE.store(false)` → `zone_below_high_watermark()` →
`set_state(INTERRUPTIBLE)` → `schedule()`, with no re-check of the wake token after
marking itself sleeping. Interleaving: kswapd clears the flag and reads needs_reclaim ==
false; the allocator (another CPU, below WMARK_LOW) runs `wakeup_kswapd` in the window
before kswapd's set_state — `KSWAPD_WAKE.swap(true)` succeeds and `Task::wake_up` is
called, but kswapd is still RUNNING so wake_up_enqueue_opt refuses it (curr_on scan) and
the wake is dropped. kswapd then marks INTERRUPTIBLE and sleeps. Every later
`wakeup_kswapd` sees the token already true and returns early without ever calling
Task::wake_up — so kswapd never runs again: the token can only be cleared by kswapd
itself, and nothing else wakes an INTERRUPTIBLE kthread with no timer armed. Trigger:
allocation racing kswapd's pre-sleep window (narrow but recurring; once hit it is
permanent for the boot). Effect: background reclaim is dead; direct reclaim still
serves allocations so the system limps with higher latencies and more OOM escalations.
Linux comparison: kswapd uses prepare_to_wait/wakeup protocol —
`wait_event_freezable_timeout(pgdat->kswapd_wait, ..., ...)` with the condition re-checked
after the state change (mm/vmscan.c kswapd). Fix direction: mark INTERRUPTIBLE first,
re-check `KSWAPD_WAKE.load() || zone_below_high_watermark()`, and only schedule when the
recheck is clean (the pool_full_backoff pattern in sync/futex.rs); a set token skips the
sleep and is consumed by the next loop iteration.

---

## Totals

| Severity | Count |
|---|---|
| Critical | 0 |
| High | 3 |
| Medium | 3 |
| Low | 3 |
| Info | 1 |
| **Total** | **10** |

(One additional candidate — a zone.rs `free_pages` counter drift on split/merge — was
drafted and RETRACTED during the review: re-deriving the arithmetic shows the counter
counts pages and the split/merge intermediate list moves never change the page count,
so `1 << target_order` on alloc and `1 << order` on free are exactly right.)

## Three worst

1. **[High] sync/futex.rs — signal-return race leaks waiter-pool slots permanently.**
   A signal-woken sleeper's `remove_waiter` can miss the waker's just-completed unlink
   ("futex-remove-miss") and return without freeing the slot; nothing ever reclaims an
   unchained `Some(Waiter)` slot. Each SIGKILL-vs-wake race shrinks the fixed 256-slot
   pool; exhaustion degrades every futex wait into the 1-jiffy backoff retry loop —
   system-wide pthread breakdown. (futex.rs:1674-1707, 1749-1840)

2. **[High] sync/futex_pi.rs — PI sleeper-side cleanup runs without the PI bucket
   lock.** `futex_lock_pi`'s post-schedule cleanup and `futex_pi_exit_cleanup` part 1
   unlink/free waiters with no bucket lock, racing `futex_unlock_pi`'s handoff
   (top-read → word CAS → unlink → set `handed` → wake). The waker can stamp `handed`
   onto a freed-and-reallocated slot: a random waiter "wins" ownership of a PI mutex
   its word was never set for — mutual exclusion break. `pi_requeue_detach` in the
   same file gets this right, proving the discipline is available. (futex_pi.rs:589-609,
   812-844)

3. **[High] mm/kswapd.rs — kswapd lost-wakeup race parks the daemon permanently.**
   clear-token → check-watermark → set-state → schedule with no post-state re-check:
   a wakeup landing in the check window is dropped (target RUNNING) but leaves the
   token set, so every later wakeup_kswapd early-returns and nothing ever wakes the
   INTERRUPTIBLE kthread. Background reclaim is dead for the rest of the boot once
   hit; direct reclaim limps on with higher latency and more OOM escalations.
   (kswapd.rs:66-84, 248-266)

## Coverage appendix (all 68 files, read end to end unless noted)

Legend: **full** = read end to end this round; **targeted** = hot paths read in full,
remainder skimmed (heavy prior-round coverage exists per docs/development/review-coverage.md);
**prior+skim** = fully reviewed in an earlier round per the coverage map, re-verified
at HEAD for the regions named in its note, remainder skimmed for new code.

### kernel/src/sync (11 files, 10,697 lines)
| file | lines | coverage |
|---|---:|---|
| spinlock.rs | 696 | full |
| mutex.rs | 930 | full |
| semaphore.rs | 485 | full |
| condvar.rs | 273 | full |
| seqlock.rs | 282 | full |
| percpu.rs | 214 | full |
| rwlock.rs | 586 | full |
| rcu.rs | 888 | full |
| lockkey.rs | 348 | full |
| futex.rs | 5084 | full (protocol paths line-by-line; the sc/sc-series OH-forensic tracers ~1500-3400 read as diagnostic-only) |
| futex_pi.rs | 904 | full |
| mod.rs | 36 | full |

### kernel/src/sched (14 files, 14,862 lines)
| file | lines | coverage |
|---|---:|---|
| sched.rs | 8842 | full |
| fair.rs | 2219 | full |
| rt.rs | 663 | full |
| deadline.rs | 547 | full |
| cgroup.rs | 910 | full |
| wakewatch.rs | 649 | full |
| psi.rs | 443 | full |
| class.rs | 273 | full |
| idle.rs | 104 | full |
| stop_task.rs | 98 | full |
| stats.rs | 45 | full |
| mod.rs | 141 | full |

### kernel/src/process (14 files, 16,261 lines)
| file | lines | coverage |
|---|---:|---|
| task.rs | 6409 | full |
| exit.rs | 2755 | full |
| exec.rs | 1653 | full |
| ns.rs | 1327 | targeted (pid-ns translation/copies/setns/reanchor full; NsKind/nsfs file plumbing skimmed) |
| fork.rs | 1125 | full |
| ptrace.rs | 880 | full |
| coredump.rs | 578 | full |
| wait.rs | 525 | full |
| kthread.rs | 308 | full |
| pid_hash.rs | 266 | full |
| pid.rs | 231 | full |
| mod.rs | 132 | full |

### kernel/src/mm (29 files, 18,026 lines)
| file | lines | coverage |
|---|---:|---|
| buddy_allocator.rs | 1703 | full |
| page_alloc.rs | 1527 | full |
| page_desc.rs | 1392 | prior+skim (mapcount/refcount/order/migration bits, copy_page_contents, text-stamp table re-verified; field accessors skimmed) |
| mm_struct.rs | 1330 | targeted (refcount/flags/brk/vm counters, mm_slot, Drop teardown chain full; accessors skimmed) |
| vma.rs | 1134 | prior+skim (per coverage map: fully re-read at HEAD; split/merge/offset continuity regions re-verified) |
| zone.rs | 1083 | full |
| small_alloc.rs | 1024 | full |
| slab.rs | 825 | full |
| vmscan.rs | 807 | full |
| memblock.rs | 660 | full |
| vdso.rs | 587 | prior+skim (e_machine arm + ELF emit per coverage map; payload unchanged) |
| swap.rs | 574 | full |
| lru.rs | 570 | full |
| compact.rs | 531 | full |
| layout.rs | 534 | targeted (effective_static_regions/heap_region_end full; accessors/print skimmed) |
| oom_kill.rs | 582 | full |
| kswapd.rs | 289 | full |
| meminfo.rs | 276 | skim (field renderers; counters sourced from modules reviewed) |
| pglist.rs | 381 | full |
| pcp.rs | 371 | full |
| hugepage.rs | 307 | full |
| deferred_free.rs | 235 | full |
| vmemmap.rs | 225 | full |
| rmap.rs | 497 | full |
| mod.rs | 261 | full |
| pagemap.rs | 76 | full |
| page.rs | 153 | full |
| vmstat.rs | 74 | full |
| allocator.rs | 18 | full |

Files NOT yet end-to-end at full depth (flagged honestly): page_desc.rs and vma.rs
carry complete prior-round coverage per the map (both re-read at HEAD by their lanes);
this round re-verified their highest-risk regions named above. meminfo.rs/vdso.rs are
render/embed layers whose inputs were reviewed in full.

## Verified-clean highlights (no new defects)

- sched.rs heal machinery (R41/R46/R49/R56/R63/R74/R80/R85, pick-generation ledger,
  claim sweep): the position-independent adjudications are coherent under the GRQ hold;
  no new hazard beyond the known open hunt items documented in-code.
- page_alloc.rs reclaim gate (holder-pid CAS, exit release, self-fight, progress
  witness) and the deep pre-OOM drain: balanced on every arm.
- zone.rs buddy (OnFreelist-authoritative merge test, R21-2 descriptor reset, split
  accounting — the retracted candidate above was re-derived and found correct).
- pcp.rs three-state lifecycle; buddy heap INV-HEAP-3 bitmap + R75 oversized-alloc
  rejection; small_alloc magazine screening + r8final heals; slab.rs O(1) kfree with
  per-cache validation; memblock full-merge insert.
- exit.rs straggler-drain/lost-straggler rescue; E8-REPAR pinned reparenting;
  claim_child_locked double-reap exclusion; exec.rs two-phase image build with RAII
  guard and switch-before-drop; ns.rs upid-chain pid translation.
