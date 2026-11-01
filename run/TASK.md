# phantom-bits lane — KILL the phantom preempt-bit producer

Mission (coordinator, 2026-11-01): the OH launcher's last kernel bug.
Sharpest signature: lb60 dump @169.4s — woken FFRT pair (340/341,
vruntime ~1e6) QUEUED tens of seconds while incumbent pid 552
(os_ds_service, preempt=0x100, vruntime ~1.05e9) ran; one SOFTIRQ-bit
monopolist strands the whole ladder.

## VERDICT (first successor, 2026-11-01) — the "phantom bookings" were
## LIVE brackets; two real engines found and killed

1. The 0x100 monopolist face = a task stuck INSIDE an inline
   __do_softirq window. Entry asm keeps IF=0 through the whole trap
   bracket (trap.S has no sti) → the window is UNTICKABLE (no tick →
   no PREEMPT-HEAL, no orphan scan, no preemption). Two engines:
   (a) irq_exit dispatched softirqs inline over ANY spinlock holder —
       the handler spins on the same lock the interrupted task holds,
       same CPU, unbreakable (the lb55/56 both-CPU UNIX_TABLE DEADLOCK
       family, census pc=0x101/0x102);
   (b) ethernet_poll's per-handler drain was UNBOUNDED (loopback
       request/response feeds itself during the drain; the 2 ms budget
       only checks BETWEEN vectors) — seconds-long windows (lb60 552:
       22 s+ at pc=0x100 pure).
   ZERO sub=0x100 underflows + silent SOFTIRQ-BRACKET-LEAK all boot
   proved the release never MIS-LANDED — the window never finished.

2. The +1 "phantom base counts" (was=0x10001 x674/boot heals,
   sub=+1 underflows) = the tick-side PREEMPT-HEAL's CUMULATIVE bar
   false-adjudicating LIVE guards of lock-hot tasks. pb3 evidence:
   887 heals/boot, 96% on 4 lock-hot pids (os_ds_service, mmi_service,
   ...), each healed every ~2 s ALL BOOT, every HEAL-ORIGIN line with
   oage_jif=0-1 (a FRESH 0->1 acquire inside the "2 s hold" — the base
   drains between ticks; the 100 Hz sampler never catches the gaps).
   Each false heal stripped a live guard + set need_resched → the
   holder preempted MID-SECTION → its lock stranded → spinners on both
   CPUs (the DEADLOCK cascade) → their own windows matured → cascade.
   The underflows are the stripped guards' drops landing on zero.

## Fixes (all on origin/phantom-bits)
- 29a27887 dfx: softirq-bit shadow (ssite) + tick-side BIT-ORPHAN scan
  + PREEMPT-HEAL-SOFTIRQ site line (dfx=leakhunt-gated).
- 2efa8981 dfx: per-CPU softirq window record (SOFTIRQ_WIN/T0) +
  WAKE-STRANDED census sq= line for windows >50 ms.
- 1854ec4d fix: irq_exit inline dispatch now requires base==0 (defer
  to ksoftirqd otherwise); ethernet_poll bounded at 64 pkts/source
  with NetRx re-arm; ksoftirqd yields on need_resched between windows.
- d2381bf9 fix: heal contiguity bar — per-task ti_pc_zero_jif stamps
  every REAL outermost base drain (sub path); the heal re-arms on a
  recent drain. Direct zeroings don't stamp → genuine phantoms still
  heal after a contiguous 2 s.
- (hatch) fix: 20 ms starvation bound on the deferral (SOFTIRQ_WIN_T0
  = last-dispatch stamp; force inline when starved) — bio/queue
  waiters busy-poll with lock_irqsave held and their completions ARE
  the deferred softirqs (make-test VW-WEDGE disk=0 irqs=0 caught it).
- (halt-drain) fix: make-test halt path drains pending softirqs before
  the WFI loop (ksoftirqd can never run in a pure WFI loop).

## Verification (OH image, x86_64 KVM, USERDATA=userdata-fresh2)
- pb1/pb2 (taps only): reproduced the wedge (43-55 s txn hangs,
  125 s strand, census softirq incumbents pc=0x102).
- pb3 (softirq fixes): softirq monopolist faces GONE (no sq= windows
  >50 ms, no DEADLOCK); heals/underflows UNCHANGED (273/82 @180 s) —
  the +1 family was independent → decoded as false heals (see 2).
- pb4 (ALL fixes, 1500 s full boot): heals 887→0, underflows →0,
  WAKE-STRANDED 1 (2.1 s boot transient), DEADLOCK 0, BIT-ORPHAN 0.
  The phantom-bit layer is DEAD. NOTE: the samgr/installs 30 s-class
  txn hangs PERSIST (avg 81 s @300 s window) with ZERO heals — the
  remaining hang carries NO preempt-count/monopolist face; it is a
  different (userspace futex chain / fair-pick) family for the OH lane.

## Gates
- make test: ALL TESTS PASSED (riscv64). x3 builds rc=0 (x86_64
  debug+release, riscv64 debug+release, aarch64).
- KNOWN COSMETIC: post-verdict halt-time VW-WEDGE spam on riscv64
  make-test boots (device completed, used ring full, PENDING_LAST=0,
  irqs=0 — halt races the last flush; verdict green, run.sh accepts
  the timeout kill; the halt-drain fix above addresses the deferred-
  ksoftirqd half). Base commit does not show it; 4/4 tap-carrying
  boots did pre-halt-drain.

## Rig
python3 run/lb-boot.py pbN 1500 run/system-lb.img \
  target/x86_64-unknown-none/release/bzImage \
  " dfx=wwcensus,leakhunt,txnhunt LOGLEVEL=6 SMP=2"
with USERDATA=run/userdata-fresh2.img. Images are SYMLINKS into
/home/william/rux-agents/oh-launcher/run (never copy image bytes).
One QEMU at a time; kill only ours (identify via /proc/<pid>/cmdline
bzImage path — serial is stdio so no tag in cmdline); pkill FORBIDDEN.

## Next lane notes
- The monopolist/phantom layer measured dead; OH ladder still blocked
  by the installs/samgr futex-chain hang (TXN-HANG-OUT exec>1.3e6 ms
  on pb4) — hunt from the TXN-HANG-STK taps (oh-ladder), not from
  preempt counts.
- WW3's 1 Hz census always shows pc=256/0x10000 — SELF-OBSERVATION
  artifact (the heartbeat runs inside a softirq window). Do not read
  it as monopolist evidence.
- Stretch (kpreempt=1 re-eval): with false heals dead, the false-heal
  deadlock objection (lb19b) is gone; re-trial kpreempt=1 next session.
