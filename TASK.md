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
- allocwatch/syncguard/lostpark/etc: map each bespoke instrument to the
  standard events that will replace it; port when the hunting lanes
  close their rounds.
- The `dfx=` cmdline keeps working (legacy shim reading the same
  AtomicBool table).

## Method
- Read the reference implementation shape first (tracepoint site
  macros, event registration, ring layout, tracefs file semantics);
  replicate semantics exactly, keep our internal implementation free.
- Boot on x86_64+KVM to exercise: echo > set_event, cat trace, marker.
- x3 builds rc=0 + make test green before every push (origin/dfx-trace;
  kill your suite's lingering riscv64 QEMU after make test — it holds
  rootfs.img's write lock; match by exact cmdline).

## Session log (append per session)
