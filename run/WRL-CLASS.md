# WRL-CLASS — the two remaining kernel-side OH suspects: WAKE-RUN-LEAK census anatomy + the voluntary-exit-owing-txn verdict

Session: 2026-11-16. Worktree `/home/william/rux-agents/wrl-class`, branch
`wrl-class` @ 3369872fc (current main, fuxwatch reopen fix in) + the two
instrument fixes of §5 (committed on the branch). Vehicle: the neverword f2
recipe on current main — KVM q35, SMP8/8G, private 16G copy of pristine
`userdata-f16g-fresh3.img`, NOSNAP first-boot population,
`dfx=pctlmon,fuxstuck,txnhunt,bindertrace,periodic trace_event=input`,
540 s windows, **without** parkdump (the lr-series serial corruption source).
Boots: w1 (early face, kernel = plain main), w2 (ladder-ish face, kernel =
plain main, the item-2 jackpot), w3 (fixed kernel, instrument validation).

## 0. TL;DR

1. **WAKE-RUN-LEAK is a benign re-park census, not a wake leak.** Every row
   on current main (w1: 185 full rows / 85 tids; w2: 293 / 170; lr-f2
   historical: 573 / 260) is a binder reader parked ≥15 s in ONE
   BINDER_WRITE_READ ioctl whose census kick **did run it** — it re-checked
   `thread_has_work`, found nothing readable, and re-parked *inside the same
   wait_event epoch*. Proof: per-tid `sum_exec_runtime` advances between
   censuses for every multi-row tid with a CFS policy (w1: 46/46; w2:
   79/81 — the 2 exceptions are SCHED_FIFO threads whose CFS entity is
   never accounted, a read-side artifact, §2.4); every row has wok≥1 (the
   wake path transitioned the queue entry); the census's own WW-PROBE
   re-wakes the task live and observes ret=true + linked 0→1 (w1 159/162,
   w2 214/216). **Zero never-ran tasks in the family.**
2. **The chain participants ARE in the family — and that is expected, not
   damning.** A frozen sync-IPC chain parks BOTH ends in the binder read
   (client waiting for the reply, owing looper waiting for work); the census
   kicks them, they run their re-check, re-park. lr-f2: accountmgr main
   (tid 716, the StartUser caller) and foundation tid 975 (txn 4570's
   owner); w1: tid 419 owing txn913; w2: samgr main (327), foundation main
   (728), and txn20768's client tid 649 — all benign re-park shape. Their
   problem is missing WORK (a reply that never comes), not a missing WAKE.
3. **The counting semantics have drifted in NAME only.** `rekick_count`
   resets per park epoch (per BWR ioctl); a row means "kicked ≥1 scan ago,
   still parked in the same ioctl" — for an idle reader that is the normal
   condition, so ~550 rows/boot of noise print under the "LEAK" name. A
   genuinely parked-never-woken task would show wok=0 or frozen exec —
   none exists in any sampled boot. Fixed the instrument to say so
   (`shape=re-park|never-ran|rt-n/a`, §5).
4. **Item 2 verdict: the kernel-side exit path CANNOT bypass txn
   completion — Rux already exceeds the Linux reference — and the f2
   "handler exits owing txn 4570" row was an instrument artifact.**
   Linux v6.1 has NO task-exit hook: `binder_thread_release` runs only at
   BINDER_THREAD_EXIT ioctl or fd close (process death), and OH libbinder's
   normal worker exit does NOT send BINDER_THREAD_EXIT (verified in
   oh-robot source) — on the reference, a thread that exits owing leaves
   the sender frozen until process death. Rux funnels EVERY exit
   (SYS_exit, exit_group, fatal signal) through `do_exit →
   binder_task_died → binder_reap_thread`, which fails every in-flight and
   queued txn to its sender (BR_DEAD_REPLY staged + exact-thread wake) —
   and w2 proves it live: the three THREAD-DIED-SERVICING exits (txns
   1527, 1864, 8762) left ZERO hang rows — their clients were unstuck.
   The f2 attribution error: UIEXIT's `owing_txn` is tgid-wide with a
   silent fallback — txn 4570's real owner (tid 975) never exited (alive
   in census rows to 424 s); the exiting tid 1523 owed nothing. w2
   reproduced the artifact wholesale: 9 foundation siblings exited clean
   at 241.8 s each printing owing txn20768 (owner tid 1189, alive, reply
   deferred userspace-side), and render_service's group death printed the
   same owing_txn=6296 on 28 different threads. Fixed the print (§5).
5. The launcher wall stays where neverword left it: image-side frozen
   sync-IPC chains (live handlers that defer replies forever). No kernel
   lost-wake, no exit-owing bypass, no corpse class survived scrutiny.

## 1. Rig and boots

- Build: `PLATFORM=x86_64 make build-x86-release` rc=0 @ main 3369872fc
  (run-build-x86-rel.log); w3 used the fixed kernel (run-build-x86-fix2.log).
- Driver: run/wrl-boot.sh → run/lb-boot-qmp.py (same as neverword nw-boot).
  Images symlinked to /home/william/rux-images/oh/; per-boot private
  userdata copies (sparse ~45M, deleted after each boot).
- w1: early face (netmanager chain: txn460 pid526 tid609 → pid327 tid357
  parked in recv(); txn913 pid512 tid700 → pid406 tid419) — foundation
  never started. 0 panics, 0 ExecReboot.
- w2: UI-phase face — render_service (727) group exit at 47.8 s, foundation
  (728) alive to window end owing txn20768 (client pid511 tid649, server
  tid1189 parked in BWR, exec frozen = never scheduled again = userspace
  deferral). 0 panics, 0 ExecReboot. 96 UIEXIT rows, 38 with owing_n>0,
  3 THREAD-DIED-SERVICING.
- w3: fixed-kernel validation boot (§5).

## 2. Item 1 — WAKE-RUN-LEAK census anatomy

### 2.1 What the census is (counting semantics, item c)

`stall_report()` runs on the khungtaskd loop every 60 s
(HUNG_TASK_CHECK_INTERVAL_SECS = 120/2). For every binder pool thread with
`read_wait_since != 0` (parked in the blocking read of BINDER_WRITE_READ)
and age ≥ 15 s it kicks the thread (`wake_thread`) and bumps `rekick_count`.
The wait queues involved are the per-thread kernel wait queues of the
binder pool (`BinderThread.wait`) — NOT futexes and NOT the proc todo:
every census row is a thread parked in the binder read ioctl itself.
The kick outcome counters (wok/wno) count transitions of that queue.
The counters (`rekick_count`, `rekicked`, `kick_wake_ok`, `kick_wake_refused`,
`read_wait_since`) reset ONLY when the thread parks in a NEW ioctl
(binder.rs, the reset block before `wait_event_interruptible!`). A row
prints when a kick happened ≥1 scan ago and the thread is STILL parked in
the same epoch. Since the wait_event macro re-checks the condition and
re-sleeps INTERNALLY (without returning to userspace), a legitimately idle
reader — no readable work — stays in the epoch forever and prints a row per
scan: kicked 2-8x, ages 80-480 s. That is the whole family. A true leak
(kick accepted, task never runs) would show wok≥1 with frozen exec and
would repeat identically every scan — it does not occur (§2.2).

Two earlier anomaly candidates dissolved under clean-row scrutiny:

- **"exec drops / counters decrease mid-epoch"** (e.g. lr-f2 tid 716/668,
  w1 tid 345 first-sample exec_ns=12): serial interleaving truncation and
  mis-joins — the complete raw rows show monotone values (w1 181 s scan:
  tid 345 already `wok=2 exec_ns=12.x ms`, matching all later rows).
- **"exec_ns < 1 µs rows"**: truncation again (the `exec_ns=` digits were
  cut by a concurrent print) plus, in w2, two SCHED_FIFO threads (750,
  756, `pol=1`) whose `sched_entity().sum_exec_runtime` is never accounted
  (SchedRtEntity has no runtime field) — the census reads 0 forever for RT
  threads. Not never-ran tasks: their wok advances and their WW-PROBE
  wakes them (ret=true, linked 0→1).

### 2.2 The row population (items a + b)

Per boot: 85-260 distinct tids, 185-573 full rows, ages 80-480 s, kicked
2-8x. ALL rows: state=0x1 (INTERRUPTIBLE), on_cpu=false, cfs_linked=0,
pick_state=1 (PICK_LIVE) — parked, not stranded-runnable. wok 1-11 (never
0), wno>0 on ~half (kicks refused while the task is mid re-check — the
normal race). Comms: OH IPC worker threads (OS_IPC_N_t) dominate, plus
named process mains and services — samgr, foundation, storage_daemon,
time_service, memmgrservice, accesstoken_ser, softbus_server,
distributeddata, resource_schedu, el5_filekey_man.

**Chain-participant overlap (item b): YES, and benign.** The frozen chain's
parked ends are binder readers, so they are census members with the same
re-park shape:

- lr-f2 (historical): tid 716 = accountmgr main, the StartUser caller
  parked in BWR to 511 s (client end of txn 4570); tid 975 = foundation
  OS_IPC_0 looper, txn 4570's OWNER, alive in census rows at 183/364/424 s.
- w1: tid 419 = txn913's server (parked owing, reply deferred userspace-
  side) — census member at 108 s+.
- w2: tid 327 samgr main, tid 728 foundation main, tid 649 (txn20768's
  client) — all census members.

Their kicks run them (exec advances), they re-park because the WORK they
wait for does not exist (the owed reply is deferred userspace-side by a
LIVE handler). The mission's "if the family includes them, it's NOT benign"
was the right test — the answer is that inclusion is expected: a parked
client IS an idle reader from the wake path's perspective. The freeze is
missing work, not a lost wake; the kernel wake path is exonerated on every
member, chain participants included.

### 2.3 What would have been a real leak, and why none exists

A real wake→run leak = kicks transition the wait entry (wok>0) but the task
never executes (exec frozen, state stays INTERRUPTIBLE, unlinked).
Discriminators all negative: 0 tids with frozen exec (CFS), 0 rows with
wok=0, WW-PROBE 373/378 ret=true linked 0→1 across w1+w2. This confirms
and extends the wildwrite exoneration to the OH boot's binder population:
the generic wake path is clean on current main.

### 2.4 Instrument fix landed (see §5): the row now self-adjudicates

`shape=re-park` (exec advanced since last census — benign idle reader),
`shape=never-ran` (exec frozen — the real leak class, would now be visible
in one glance), `shape=rt-n/a` (RT policy — no CFS exec signal).

## 3. Item 2 — voluntary exit holding a sync txn

### 3.1 The kernel path is complete and stronger than the reference

Code path (current main): `do_exit` (ALL exits — SYS_exit single thread,
exit_group, fatal signals) → `binder_task_died(pid, tgid, tid)`
(exit.rs:1542) → membership lookup incl. tid-scan fallback → (proc alive)
`binder_reap_thread`: drains `txn_stack` (every in-flight txn, not just the
top) and `todo`; each sync txn gets `dead_reply_to_sender`: unstack from
the sender, stage BR_DEAD_REPLY (single-slot parity with
binder_send_failed_reply), exact-thread wake, fallback to proc todo +
proc wake if the sender thread is gone; buffers freed, node pins dropped.
`BINDER_THREAD_EXIT` (musl's clean shutdown) shares the same reap.
Group death: every member funnels through do_exit individually; the fd
close (`binder_close`) drains the remainder and removes the proc.

Reference (linux v6.1 drivers/android/binder.c, fetched to
/tmp/linux-binder.c): `binder_thread_release` is called ONLY from the
BINDER_THREAD_EXIT ioctl case and from `binder_release` (fd close — which
walks every thread of the proc). It fails ONLY the TOP owed txn
(`if (t->to_thread == thread) send_reply = t` → one
`binder_send_failed_reply(send_reply, BR_DEAD_REPLY)`) and silently unlinks
deeper entries. There is NO do_exit hook in the binder driver: a thread
that exits owing without sending BINDER_THREAD_EXIT leaves the sender
parked until the PROCESS dies.

OH userspace (oh-robot): the normal IPC worker exit path
(`IPCWorkThread::ThreadHandler → BinderInvoker::JoinThread`) sends
BC_EXIT_LOOPER and returns — it does NOT send BINDER_THREAD_EXIT
(`BinderInvoker::ExitCurrentThread` exists but is only called on the
process StopWork path). So on the reference kernel, f2's shape (a worker
exiting owing) would freeze the client until foundation dies. Rux instead
fails the txn at the exit itself. Verdict: no kernel-side bypass exists;
nothing to fix in the lifecycle.

### 3.2 The f2 row was a misattribution — now proven twice

`txn_owing_of(tgid, tid)` returns tgid-wide entries with a FALLBACK: if no
exact-tid match, `own_txn` = first txn owed by ANY thread of the tgid (and
the old UIEXIT print dropped `owing_stid` entirely). lr-f2's
"UIEXIT tid=1523 owing_n=1 owing_txn=4570": the row was serial-truncated
before any stid field; the real owner of txn 4570 was tid 975, which NEVER
exited (alive in census rows to 424 s; TXN-HANG-OWE outstanding=[NONE] =
reply deferred userspace-side). tid 1523 exited clean (exit(0),
lastsys=60).

w2 reproduced the artifact at scale:
- foundation (728), 241.6-241.8 s: NINE sibling threads (1168-1176) exit
  voluntarily (cause=thread-exit, lastsys=60) — every one prints
  owing_n=1 owing_txn=20768. THREAD-DIED-SERVICING fired for NONE of them
  (own stacks empty). Txn 20768's owner is tid 1189 — alive to window end,
  parked in BWR owing since ~230 s, exec_dx=0 (never scheduled again).
  Client pid 511 tid 649 frozen = image-side deferred reply.
- render_service (727) group exit at 47.8 s: 28 threads each print
  owing_txn=6296 (one tgid-wide debt); TDS shows tid 1104 owed txn8762 of
  its own — both debts were failed to their senders by the reap (zero
  TXN-HANG rows for 6296/8762 afterwards).
- TDS rows (the authoritative own-stack signal): tgid 706 tid 706
  (stack=2, txn1527) and tid 878 (stack=1, txn1864) at 10.8 s — again ZERO
  hang rows for those txns: the clients were BR_DEAD_REPLY'd and unstuck.
  (RERR-PUSH printed nothing because that tap is accountmgr-sender-gated
  and no accountmgr debt existed in w2.)

### 3.3 Verdict

Kernel-side: nothing to fix — the completion at exit is wired, verified
live, and exceeds the reference (which the OH userspace relies on NOT
having, since its worker exits skip BINDER_THREAD_EXIT). The freeze class
that motivated the question is image-side: live handler threads that pick
up sync txns and never reply (foundation tid 1189/975 shapes). Userspace
defect documentation: an OH handler that defers a sync reply behind a
wait that never completes freezes its client indefinitely — no kernel
(mainline or Rux) cancels a DELIVERED txn while the server lives; the
reference behaviour is identical.

## 4. What was NOT found (negative results worth keeping)

- No corpse BinderThread class: every suspect "recycled pid / latched
  epoch" reading dissolved into serial truncation or the RT-exec artifact.
  The one theoretical miss path in `binder_task_died` (first-match proc
  lookup shadowing the true owner when a recycled pid equals another
  proc's stale opener pid) has no observed instance in 3 boots + lr-series
  review; noted here for the next audit, not fixed (no repro, and the
  tid-scan fallback only runs when the pid match fails, not when it
  mis-hits).
- No lost wake on any chain participant (item 1's malign class): zero.

## 5. Fixes landed on branch wrl-class (instrument-only, 2 files)

1. `kernel/src/process/exit.rs` — UIEXIT owing decode: print `owing_stid`
   and `own=1|0` (exact-tid vs tgid-wide fallback). Ends the
   misattribution class that cost the neverword lane its "sharpest
   instance".
2. `kernel/src/ipc/binder.rs` — WAKE-RUN-LEAK self-adjudication: new
   per-episode `wrl_last_exec_ns` (reset with the kick counters at park);
   the row gains `exec_dx_ns=` and `shape=re-park|never-ran|rt-n/a`
   (RT/DL exempt: no CFS exec accounting exists for them). Also corrected
   the drifted `rekick_count` doc comment. No new dfx switch; the frozen
   legacy instrument keeps its shape, it just stops lying about what a
   row means.

Both are print/diagnostic-only; no functional path touched.

## 6. Gates (merge gate, print-only changes but run anyway)

- x86_64 build (config carried): rc=0 (run-build-x86-fix2.log).
- riscv64 build: rc=0 (run/gate-riscv-build.log).
- aarch64 build: rc=0 (run/gate-aarch64-build.log).
- x86_64 clean-config rebuild (rm build/.config): rc=0
  (run/gate-x86-rebuild.log).
- riscv64 `make test`: rc=0, "All tests passed" (run/gate-riscv-maketest.log;
  the first attempt rc=2 was a missing test/rootfs.img — built with
  `make rootfs` rc=0, then the suite ran green; run/gate-rootfs.log).
- w3 boot on the fixed kernel: instrument validation (below).

### w3 validation of the fixed instrument (540 s, 0 panics, 0 ExecReboot)

- WAKE-RUN-LEAK rows with `shape=`: **331 re-park, 2 rt-n/a, 0 never-ran**
  — the classifier's whole population is benign on the fixed kernel too
  (353 raw rows; the 2 rt-n/a are the SCHED_FIFO workers of §2.4).
- UIEXIT owing rows now decode the fallback explicitly: e.g. render-family
  (tgid 761) exits print `owing_n=1 owing_txn=6976 owing_stid=0 own=0`
  (a sibling's debt, not the exiting thread's).
- samgr (327) died as a group at 168.6 s (SIGSEGV): TDS fired for tid 694
  (stack=1, txn20260 from pid 766) — and txn20260 has ZERO hang rows after
  the death: the group-death reap failed the debt and unstuck pid 766.
  Same for the two 10.9 s TDS debts (txn1743, txn2347 from pid 330 tid
  691): zero hang rows. The one hanging txn at that moment (txn19623,
  foundation→samgr tid 362, parked on a futex owing) had its last hang row
  at 166.5 s, BEFORE the group death — the death-side drain resolved it.
- Exit-owing reap end-to-end: now verified on w2 AND w3 (5 debts, 5
  unstuck clients, 0 residual hang rows).

## 7. Artifacts

- run/wrl-boot.sh, run/wrl-analyze.py — driver + analyzer.
- run/oh-w{1,2,3}-serial.log, -markers.txt, -driver.out, run/w{1,2,3}-series.out.
- /tmp/linux-binder.c — fetched linux v6.1 binder.c (reference quotes §3.1).
- Historical: /home/william/rux-agents/launcher-race/run/oh-f2-serial.log.gz
  (re-analyzed here), neverword NEVERWORD.md, wildwrite WILDWRITE.md.
