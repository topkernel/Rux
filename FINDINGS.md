# GNOME CPU storm — findings

Worktree: `/home/william/rux-agents/gnome-cpu` (branch `feature/gnome-cpu-storm`, from main @ 41d7aad1).
Runs: `runs/base1`, `runs/base2` (baseline, kernel == main), `runs/fix1` (patched kernel).
Driver: `runs/cpumin.py` (cold-copies the shared image, boots Xorg + session dbus only,
samples /proc/PID/stat, runs the pollburn repro, triggers a manual DUMP!).
Repro: `test/pollburn.c` (static riscv64 build placed in the image as /root/pollburn).

## Symptom recap (from r74j reference run)

- Almost every glib main-loop task (gmain, gdbus, gsd-*, Xorg, dbus-daemon) shows
  `state=RUNNING on_rq=1` in periodic taskdumps.
- dbus-daemon / Xorg appear to accumulate tens of seconds of vruntime per 110 s window.
- Everything uniformly slow: component starts spaced ~100 s, driver xterm ~280 s to appear.

## Root cause (two distinct mechanisms)

### 1. poll/ppoll/select busy-wait at tick rate (the real CPU burn)

`sys_poll` (kernel/src/syscall/misc.rs) has NO fd-side wake registration: when no fd is
ready it calls `poll_sleep_slice()` which sleeps ONE 10 ms re-check slice (one jiffy at
HZ=100), wakes, re-polls every fd (copy_from_user the pollfd array, run each fd's poll
callback, copy_to_user), and sleeps again — forever. Every poll()/ppoll()/select() waiter
in the system wakes 100x/second. With ~40 glib main loops in a GNOME session that is
thousands of wakeups + full fd-array re-polls per second, all serialized on the global
runqueue lock at every tick — this is the burn, and it also explains the uniform slowness
(CPUs oversubscribed by poll bookkeeping).

`sys_epoll_wait` has the same 10 ms re-check design but at least sleeps on the epoll
instance's wait queue; only unix sockets call `epoll_notify_file()` on data arrival, so
epoll over other fd types also re-checks at tick rate.

Evidence (baseline, minimal session = Xorg + dbus only, smp2, /proc/PID/stat deltas over
30 s windows, CLK_TCK=100):

- Xorg pid 307: stime 275→284→303 ticks (3-6% of one core, pure system time; utime flat).
- dbus-daemon pid 318: stime 16→29 (+13 ticks/30 s ≈ 4%).
- Xorg InputThread pid 320: stime 15→26 (+11 ticks/30 s ≈ 4%).
- pollburn busy-mode control validates the accounting: 2001 ticks / 20 s = exactly 100%.

In the full GNOME session each of ~40 waiters burns a few percent and they also contend
on the GRQ; that aggregate is the storm.

### 2. The taskdump "RUNNING" flood is a sampling artifact (plus real load)

- `sleep 120` (blocked in clock_nanosleep) correctly reports INTERRUPTIBLE(sleep) in a
  manual (serial-RX context) DUMP! — the state reporting itself is fine.
- pollburn blocked in poll()/select() on an idle socketpair shows RUNNING on_rq=1 in the
  same manual dump — because poll waiters are only ever asleep for 10 ms at a time; at
  any instant they are frequently freshly woken/runnable (and each poll re-check pass is
  expensive under TCG, so they spend real time on the runqueue).
- Periodic dumps (dfx=periodic) run from the TIMER SOFTIRQ — the exact context in which
  every 10 ms re-check timer fires — so a periodic dump sees ALL poll waiters freshly
  enqueued: that is the "everything RUNNING" signature in r74j's M5 taskdump.
- (Note: vruntime is NOT a valid burn metric on this kernel — sleeping tasks get clamped
  up to min_vruntime on wake, so all frequently-woken tasks show near-identical vruntime.
  Use /proc/PID/stat utime/stime instead.)

## Fix (this branch)

Linux-style poll_wait for poll/ppoll/epoll_wait (P3):

- New dispatch `poll_wait_impls(file)` (misc.rs) keyed on FileOps identity registers the
  WAITING TASK on the target fd's readiness wait queue:
  - unix sockets: per-socket queue, woken on peer data / accept / send-space / EOF
    (all wake sites already existed for blocked readers).
  - pipes + FIFOs: read_queue AND write_queue (producers wake both).
  - AF_INET sockets: per-socket queue (wake_tcp_matches/wake_udp_matches).
  - inotify: instance queue (event enqueue wakes it).
  - evdev: NEW per-device poll_waitq woken in push_event (device had no queue before).
  - timerfd: tfd.wait_queue (timerfd_expire_notify).
  - signalfd: sfd.wait_queue (signalfd_notify_task on signal arrival).
- `poll_event_wait_round()` (misc.rs): register on all polled files, re-check readiness
  after registration (lost-wake discipline), arm ONE timeout timer for the full remaining
  time, sleep until woken by a producer event / timeout / signal.
- sys_poll: uses the event wait; falls back to the old 10 ms slice ONLY when some polled
  fd has no wake path (exotic types) or the timer pool is exhausted.
- sys_epoll_wait: registers the waiting task on every watched file's queue (ET snapshot
  semantics unchanged); same slice fallback.
- pselect6/select: unchanged (still slice-based) — nothing in the GNOME session waits in
  select; glib/Xorg/dbus all use poll/ppoll.

## Verification results

### Isolated repro (pollburn), fixed kernel vs Linux expectations

Minimal session (Xorg + session dbus only, smp2/1G), runs/fix1 + runs/fix2:

```
POLLBURN mode=wake  poll_ret=1 revents=POLLIN latency=1.010s
                   (child writes at t=1s; parent woken instantly — the
                    producer-wake path works, no latency regression from
                    removing the 10ms re-check)
POLLBURN mode=busy   wall=20.0s cpu_ticks=2000 (100.0%)   <- accounting control
POLLBURN mode=poll   wall=20.0s cpu_ticks=1   (0.0%)
POLLBURN mode=ppoll  wall=21.0s cpu_ticks=0   (0.0%)
POLLBURN mode=epoll  wall=21.0s cpu_ticks=0   (0.0%)
POLLBURN mode=select wall=21.0s cpu_ticks=0   (0.0%)
```

(A 1s-timeout wait on an idle socketpair in a loop; on Linux this is ~0 ticks.
The OLD kernel burns at tick rate inside each wait — see the baseline numbers
below; it also cannot complete an INFINITE poll(-1) round at all, which is why
the repro uses 1s timeouts.)

fix2 steady-state deltas (ticks per 30 s window): Xorg main stime
+21/+9/+14/+10 (~3-7%... first window includes settle; utime growth 0),
Xorg InputThread 0, dbus-daemon 0. pollburn tasks blocked in poll report
INTERRUPTIBLE(sleep) in the manual dump.

### Per-process CPU, /proc/PID/stat tick deltas per 30 s window (CLK_TCK=100)

Baseline (stock kernel, runs/base2, minimal session):
- Xorg main:     stime +9/+19/+13/+15 per 30s (3-6% of one core, utime flat)
- dbus-daemon:   stime +13/+14/+11/+8  (~4%)
- Xorg InputThread: stime +11/+13/+15/+20 (~4-6%)

Fixed (runs/fix1, same session):
- Xorg main:     stime +23(first)/+8/+4/+14 (steady ~1.3-4.6%)
- dbus-daemon:   stime +0/+0/+0/+0  (0%)
- InputThread:   stime +0/+0/+0/+0  (0%)

Goal was < 10% each: PASS (dbus/InputThread exactly 0; Xorg main's remainder
is its own periodic work / one non-wired fd slice fallback).

### Taskdump state check

Manual DUMP! (serial-RX context, not the timer softirq):
- stock kernel: `sleep 120` -> INTERRUPTIBLE(sleep) (correct); pollburn
  blocked in poll()/select() -> RUNNING on_rq=1 (the artifact: poll waiters
  are only asleep 10ms at a time).
- fixed kernel: pollburn blocked in poll() and epoll_wait() both report
  INTERRUPTIBLE(sleep) on_rq=0. Xorg sleeping at sample time.
- Conclusion: the r74j "everything RUNNING" taskdump flood was mostly the
  timer-softirq sampling artifact over a real tick-rate wakeup loop. With
  event-driven waiting both the loop and the artifact are gone. The state
  reporting code itself was never broken.

### Full GNOME session

- runs/full1 (fixed kernel): chain ran ~10x faster than r74j (30+ processes
  within 97s vs ~100s per component step on stock main), then hit a kernel
  panic during the gsd fork/exec storm: R46/R56/R74 scheduler heal-path storm
  (RUNNING-PHANTOM/SLOTLESS-TP) followed by a wild-epc trap — the
  pre-existing pick-race family documented in sched.rs (r74a "double-run
  KERNPANIC" captures predate this work). r74j (stock) shows the same heal
  classes firing (1x R46, 8x R56, 339x PICK-SKIP-ALL) at its slower pace.
- runs/full2: survived the fork storm, ran 452 s (Xorg + 2x dbus-daemon +
  gnome-session + gsd daemons + gmain/gdbus/dconf threads, pids to ~580),
  then panicked in the uaccess copy path during heavy process churn
  (component sig=9 teardown storms — the same kill pattern r74j logged at
  t=236s/415s/641s on stock).
- runs/full3: no panic, but wedged at ~t=300 s (serial silent, QEMU at 270%
  host CPU = guest CPUs busy-spinning). khungtaskd reported 8 freshly
  spawned tasks (pids 576-599) blocked >120 s while the pick loop logged
  PICK-SKIP-ALL for exactly those pids (`oncpu=0 claim=-1` — skipped by
  every CPU, never scheduled). This is the pre-existing pick-skip
  starvation / heal-path family in sched.rs, amplified by the much higher
  event rate the session now generates. Full3's per-process samples before
  the wedge showed the same shape as full2: heavy CPU only during the gsd
  spawn/timeout/kill churn (gnome-session ~3-37%, Xorg ~2-11%), trending
  down between churn bursts.
- Component behavior otherwise matches r74j: gsd daemons start but miss the
  gnome-session registration timeout (same WARNING lines as the stock run).
- Minimal-session runs (fix1) were 100% stable through the whole scripted
  sequence (boot, Xorg, dbus, 5 sampler rounds, 5 pollburn modes, DUMP!).

## Open items / next experiments

1. Full-session stability on the fixed kernel: 3 runs gave 1 scheduler-race
   panic (t=97s, R46/R56/R74 heal storm), 1 uaccess-during-churn panic
   (t=452s), 1 pick-skip starvation livelock (t=300s, tasks 576-599 never
   scheduled while older tasks spun). All three are shapes the in-tree
   heal machinery in kernel/src/sched/sched.rs already documents (R46/R49/
   R56/R74 captures predate this work; r74j stock shows the same classes
   firing), but the event-driven poll change shifts timing and may make
   them more likely at the session's new, much higher event rate.
   Suggested next experiments, in order:
   a. Instrument PICK-SKIP-ALL to also dump the skipped task's pick_state
      and ti_kernel_sp when the skip count for one pid crosses ~1000 —
      catches the full3 starvation shape with hard data.
   b. Bisect the wake sources: wire only unix sockets first (the dbus/Xorg
      path), keep the 10ms slice for pipe/fifo/inotify/evdev, and see which
      wiring step destabilizes the session.
   c. Compare a full session on stock main driven from the SAME fast
      auto.sh (no SLEEPs) — stock main may wedge too once the pace rises;
      that would pin the instability on pace, not on the poll change.
2. gsd registration timeouts (session-level): gsd daemons start under the
   fixed kernel but still miss the gnome-session registration timeout;
   worth checking gsd-side logs (dbus activation?) next.
3. pselect6/select still use the 10ms slice fallback (nothing in the GNOME
   chain waits in select; glib/Xorg/dbus all use poll/ppoll/epoll).
4. eventfd/netlink/raw: socket.rs wired via per-socket queue; eventfd has
   no queue -> slice fallback (unused by this session).

## Repro / how to rerun

```
cd /home/william/rux-agents/gnome-cpu
riscv64-linux-gnu-gcc -static -O2 -o /tmp/pollburn test/pollburn.c
cd runs && python3 cpumin.py <name> 1300   # boots Xorg+dbus, sampler, pollburn, DUMP!
grep POLLBURN <name>/serial.log
```

pollburn modes: busy (accounting control), poll/ppoll/epoll/select (1 s-timeout waits on
an idle socketpair in a loop — on Linux ≈ 0 ticks), wake (child writes after 1 s while
parent blocks in poll(10 s) — measures producer-wake latency; expect ~1 s, 10 s = missed
wake).
