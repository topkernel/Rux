# dfx-trace lane: unified trace-event framework, fully aligned with the reference kernel

## Charter (user directive 2026-11-02)
"Your dfx has many points — implement them fully aligned with the
reference kernel's trace events; the current bespoke-switch approach is
too messy." Build the unified framework and migrate onto it. The 57
bespoke `dfx=` switches stay working as legacy compat while other lanes
finish their hunts, but NEW observability stops adding bespoke switches
and adds tracepoints instead.

## What "fully aligned" means (concrete)
1. **Tracepoint sites**: static event sites with reference naming
   (subsystem:event like sched:sched_switch), disabled cost = one
   relaxed atomic load per site (same class as the existing switch
   table), enabled via the event table.
2. **Per-CPU ring buffers**: SPSC per-CPU rings, global tracing_on
   master switch, overwrite semantics, timestamps in the reference's
   spirit (local clocks acceptable round 1, note the divergence).
3. **Consumer interface = the reference's file layout and semantics**:
   /sys/kernel/tracing/{available_events,set_event,tracing_on,trace,
   trace_pipe,trace_marker,options,...} and
   events/<subsys>/<event>/enable per-event files. Text formatting on
   read (that is also what the reference does). If mounting a real
   tracefs needs filesystem support we lack, implement the tracefs
   mount — the paths and behaviors must match, not a /proc lookalike.
4. **First event wave** (semantics must match the reference events):
   sched family (sched_switch, sched_wakeup, sched_wakeup_new,
   sched_process_fork, sched_process_exit, sched_migrate_task),
   irq/hardirq entry-exit context markers, printk:console, and
   raw_syscalls:sys_enter/sys_exit (this subsumes the SysRing forensic
   ring long-term).
5. **Overhead gate** (guarded domains sched + interrupts): disabled
   sites = one load (verify, not assume); ENABLED overhead measured and
   reported vs disabled in syscost/pbench terms; census bands and
   pbench P2 within baseline band with events off. Image size delta
   reported (the 12MB kernel-image window is tight — kallsyms history).

## Migration ledger (round 2+, do not delete instruments mid-hunt)

The bespoke `dfx=` switch table (59 entries, kernel/src/dfx/switches.rs)
is FROZEN legacy: it keeps working untouched while hunting lanes close
their rounds, and new observability goes through tracepoints instead
(AGENTS.md policy). Replacement map:

| bespoke instrument(s) | replacement in the unified framework | status |
|---|---|---|
| schedtrace, schb, periodic, taskdump, parkdump, sleepsnap | sched:sched_switch / sched_waking / sched_wakeup / sched_wakeup_new / sched_process_fork / sched_process_exit / sched_process_wait / sched_process_free / sched_migrate_task (all present) | ready to port |
| syscalltrace, sysforensic, sysring | raw_syscalls:sys_enter / sys_exit (present; sysring forensic ring is subsumed long-term per charter) | ready to port |
| sigtrace | signal:signal_generate (present) | ready to port |
| bindertrace, dbtrace, txnhunt | binder:binder_transaction / _received / _alloc_buf / _buffer_release / wait_for_work (present) | ready to port |
| memwatch | oom:mark_victim (present) + reference mm event family (mm_page_alloc/free, round-2 events) | partial |
| allocwatch, leakhunt, mmforensics, vmap, hole, poolcensus, monowatch | reference kmem:kmalloc/kfree + mm_page_* family (round-2 events; kmem exists on the parked feature/dfx-align branch, port after reconciliation) | round 2 |
| syncguard, syncwatch, thpark, futextrace | reference lock:lock_acquire/lock_release + futex event family (round-2 events) | round 2 |
| sfddbg, exthunt, filerd | reference ext4:/jbd2: event family (round-2 events) | round 2 |
| epollscan, epolltrace | no reference analog; round-2 rux: epoll tracepoints in the unified table | round 2 |
| tcptrace, socksnap | reference net event family (tcp:*, round-2 events) | round 2 |
| inputtrace | reference input event family (round 2) | round 2 |
| writetrace | exceptions:page_fault_kernel (present) + block:block_rq_issue/complete (present) | partial |
| execcost, ohhunt, treehunt, driftwatch | sched:* + raw_syscalls:* timelines answer the same questions (present) | ready to port |
| acqdiv, scmtap, datatap, ksprobe, ripsample, steptrace, claiminject, oncpuinject, epollscan-tap | debug taps without a reference event analog; keep frozen until their hunts close, then retire with the hunt | keep frozen |
| scribble, scribblesweep, scribblepark, wildwatch, wwcensus | wild-write TRIPWIRES (active detectors, not passive observability) — no reference event analog; keep as forensics, frozen | keep frozen |
| watchdog, softlockup, hung_task | DFX SAFETY NETS (act, don't just observe) — out of the tracepoint migration by design | permanent |
| ledger, ledger-bookkeeping | internal accounting of the above, not an instrument | n/a |

## Method
- Read the reference implementation shape first (tracepoint site
  macros, event registration, ring layout, tracefs file semantics);
  replicate semantics exactly, keep our internal implementation free.
- Boot on x86_64+KVM to exercise: echo > set_event, cat trace, marker.
- x3 builds rc=0 + make test green before every push (origin/dfx-trace;
  kill your suite's lingering riscv64 QEMU after make test — it holds
  rootfs.img's write lock; match by exact cmdline).

## Round 1 verification record (2026-11-02/03, x86_64+KVM, this host)

- **Disabled cost**: VERIFIED IN RELEASE ASM — each off site compiles
  to one `movzbl <enabled>` + `test` + `je` (syscall entry at
  ffffffff8056102d, exit at ffffffff805610a7 in the round-1 build);
  tracing_on moved inside record() — the reference's split (static key
  at the site, ring reserve under the master).
- **Enabled overhead**: guest differential, 4000x8K zero-reads, 3 reps
  each way, min-of: off 957.7-986.7 µs vs on 1107.8-1119.2 µs →
  **~37 ns/read = ~18-19 ns per record** (sys_enter+sys_exit armed).
  (run/tr1-kvm-proof-serial.log, [P5].)
- **Image delta**: x86_64 release bzImage 9,904,128 (origin/main) →
  9,916,416 (round 1) = **+12,288 bytes (+0.12%)**.
- **Serial proof** (run/tr1-kvm-proof.sh + run/tr1-proof-init.c on a
  static-init ext4 image, run/tr1-rootfs.img): P1 layout OK at
  /sys/kernel/tracing AND /sys/kernel/debug/tracing (same instance);
  P2 catalog/format files reference-shaped, bare-name set_event arms a
  whole group, unknown token = EINVAL; P3 interleaved trace with
  correct fields — sched_switch state letters (R/S/Z), a natural
  sched_migrate_task (orig_cpu=0 dest_cpu=1), irq_handler_entry
  "irq=11 name=virtio-pci" with d.h.. hardirq flags, printk:console
  from a /dev/kmsg write, tracing_mark_write markers, full
  fork→wait→exit(Z)→free lifecycle; trace_pipe consumes; per-CPU stats
  report entries/overrun/read-events (overrun 23018 under the P5 flood
  — the overwrite path exercised); P4 options read the reference
  defaults and context-info=0 drops the header.
- x3 builds rc=0; make test 1297/1297 (riscv64).
- Known divergences (all noted here, none silent):
  - printk:console records the Rux printk STORE text; /dev/kmsg writes
    carry Rux's "[kmsg]" origin tag in that text (reference stores the
    raw text).
  - Dynamic-array events (console, irq name) cap at 80 bytes per
    record (fixed-size ring record).
  - The x86 legacy-PIC line handlers are void; their irq_handler_exit
    always reports ret=handled.
  - The x86 LAPIC timer tick and the R83 scribble-heal stamp do not
    fire tracepoints (no action / reference uses untraced
    __set_task_cpu there).
  - options/ carries only the four options the renderer and ring
    consume (context-info, overwrite, sym-addr, sym-offset);
    tracer-specific reference options are omitted, not faked.

## Session log (append per session)

### 2026-11-02/03, round 1 (this session)
- Branch dfx-trace off origin/main; commits: tracefs mount + event wave
  + one-load sites, then the reference-exact set_event grammar.
- Found substantial prior trace work already on main (tracepoint!
  table, rings, tracefs files under debugfs) — round 1 completed the
  charter's missing pieces: /sys/kernel/tracing as a real tracefs
  mount (+ the debugfs alias of the same instance), irq/printk/migrate
  events, options/, one-load sites, the reference set_event grammar,
  and the whole verification battery above.
- feature/dfx-align (local branch, 19 commits over main) carries a
  kmem event wave but is entangled with other lanes' vmscan/ext4/perf
  merges — NOT merged here; reconcile in round 2 when porting the
  kmem/mm families.
- Artifacts: run/tr1-kvm-proof.sh (KVM driver), run/tr1-proof-init.c +
  run/tr1-rootfs.img (static-init proof image — mkfs WITHOUT
  metadata_csum_seed/orphan_file, the ext4 gate rejects those),
  run/tr1-kvm-proof-serial.log (full transcript).
- test/tracefs-smoke.sh updated: per-event set_event writes (the
  reference applies ONE token per write — trace_get_user).
