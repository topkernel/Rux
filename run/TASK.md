# phantom-bits lane — KILL the phantom preempt-bit producer

Mission (from coordinator, 2026-11-01): the OH launcher's last kernel
bug. Sharpest signature: lb60 dump @169.4s — woken FFRT pair (pids
340/341, vruntime ~1e6) QUEUED tens of seconds while incumbents
(pid 552 os_ds_service preempt=0x100, vruntime ~1.05e9) run; one
SOFTIRQ-bit monopolist strands the whole ladder (551 tasks preempt=0).

## Facts established this session (read before anything)
- Evidence: /home/william/rux-agents/oh-launcher/run/oh-lb60-serial.log
  (dump at line ~12183; WAKE-STRANDED 552 at 172.8s).
- lb60 boot had leakhunt DARK (txnhunt+wwcensus only): the silent
  SOFTIRQ-BRACKET-LEAK / IRQ-PAIR-SPLIT probes prove nothing.
- lb60 tallies: 943 PREEMPT-HEAL (was=0x10001 x674, +1..+4 base counts
  under the tick's own 0x10000), 293 UNDERFLOW all sub=+1 (double-drop
  at SpinlockGuard::drop spinlock.rs:522 / rwlock.rs:556), ZERO
  sub=0x100 underflows → the monopolist's -0x100 NEVER EXECUTED ON ANY
  TASK. Not a mis-landed release: the release is skipped entirely, or
  the ADD double-runs.
- Only two +0x100 bookers exist on this tree: __do_softirq's bracket
  (softirq.rs:203 add / :290 sub, no early return between) and
  SeqLock::write (seqlock.rs:137/drop, irq-off section). lock_bh /
  read_lock_bh / write_lock_bh have ZERO callers.
- The tick-side PREEMPT-HEAL observes via the SCHED slot
  (cpu_state.current); bookings resolve via the ARCH slot (tp). They
  diverge in the switch-window/abort families. The heal never fired on
  pid 552 despite a 19+ s 0x100 hold → heal-blindness is real; the
  orphan scan reports slotdiv= for exactly this.
- Monopolist mechanics (confirmed): __schedule refuses to switch out a
  nonzero-count task (sched.rs:2415) → a phantom 0x100 holder that tries
  to sleep is bounced to RUNNING and poll-loops forever, CFS-proof.

## Tap landed (commit 29a27887)
- leakorigin softirq shadow (ssite/sjif/scpu/smis) + report_softirq_origin
  / report_hardirq_origin; BIT-ORPHAN tick scan (preempt.rs
  tick_orphan_scan, called from scheduler_tick, dfx=leakhunt-gated);
  PREEMPT-HEAL-SOFTIRQ site line at heal time; ssite on LEAK-ORIGIN.

## Repro rig
- Boot: cd /home/william/rux-agents/phantom-bits && python3 run/lb-boot.py
  pb1 900 run/system-lb.img target/x86_64-unknown-none/release/bzImage
  " dfx=wwcensus,leakhunt,txnhunt LOGLEVEL=6 SMP=2" with
  USERDATA=run/userdata-fresh2.img env. Copy lb-boot.py from
  oh-launcher/run + symlink images (never copy real image bytes; images
  live in oh-launcher/run — symlink only; boot uses snapshot+locking=off).
  KVM. One QEMU at a time (ps first); kill only ours; pkill FORBIDDEN.
- The first BIT-ORPHAN line after boot = the producer's capture
  (pid + ssite file:line + sage + slotdiv).

## Gates before push
x3 builds rc=0 (x86_64, riscv64, aarch64) + make test green; small
commits, English, NEVER "Linux" (say "the reference kernel"). Push
origin/phantom-bits. Success metric: BIT-ORPHAN → 0, PREEMPT-HEAL
strips → 0, monopolist census gone, CreateNewUser 3x10s futex chain
compresses (TXN-HANG tap is the before/after instrument).

## Stretch
With FACE B dead, re-evaluate kpreempt=1 (re-land conditions in
arch/x86_64/trap.rs irq_exit_maybe_preempt_kernel).
