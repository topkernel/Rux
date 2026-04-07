# DFX Subsystem Refactoring Plan

## 1. Overview

DFX (Debug, Fault, and Diagnostics) is a unified diagnostic subsystem for the Rux kernel. It consolidates scattered debugging features into `kernel/src/dfx/` and implements critical missing capabilities for production kernel debugging.

This document covers the gap analysis with Linux, module design, and implementation plan.

---

## 2. Linux DFX Subsystems vs Rux Status

### 2.1 printk / Log Levels

| Aspect | Linux | Rux |
|--------|-------|-----|
| Ring buffer | Lockless multi-CPU ring buffer | Mutex-protected fixed array |
| Log levels | 8 levels (EMERG through DEBUG) | 8 levels — fully implemented |
| Console filtering | Runtime adjustable `console_loglevel` | `CONSOLE_LOGLEVEL` AtomicU8, adjustable via syslog(2) |
| Userspace read | syslog(2), `/dev/kmsg`, `/proc/kmsg` | All three implemented |
| Persistent log | pstore / ramoops | `/var/log/kmsg` on ext4 (disabled) |
| **Status** | — | **DONE** — `kernel/src/printk.rs` |

### 2.2 panic / oops

| Aspect | Linux | Rux |
|--------|-------|-----|
| Register dump | Architecture-specific `show_regs()` | Inline asm in panic handler |
| Stack backtrace | `dump_stack()` → arch `show_stack()` | Frame-pointer walk in panic handler |
| Taint tracking | `tainted` bitmask, `add_taint()` | **Not implemented** |
| Notifier chain | `panic_notifier_list` | **Not implemented** |
| All-CPU backtrace | NMI-based `trigger_all_cpu_backtrace()` | Stub only (`irqdesc.rs`) |
| Crash kernel | kexec / kdump | **Not applicable** (no 2nd kernel) |
| panic_timeout | Auto-reboot after N seconds | **Not implemented** (wfi forever) |
| **Status** | — | **Partial** — `main.rs:729-870`, not reusable |

### 2.3 dump_stack()

| Aspect | Linux | Rux |
|--------|-------|-----|
| Standalone API | `dump_stack()` callable from anywhere | **Not available** — code is inline in panic handler |
| Output format | CPU/PID/Comm header + call trace | Raw address list only |
| **Status** | — | **Missing** — must extract from panic handler |

### 2.4 BUG / WARN Macros

| Aspect | Linux | Rux |
|--------|-------|-----|
| `BUG()` | Fatal, calls panic("BUG!") | Rust `panic!()` serves as equivalent |
| `WARN()` | Non-fatal, logs + stack trace + taints | **Not implemented** |
| `WARN_ON_ONCE()` | Fires only once per boot | **Not implemented** |
| `BUG_ON()` / `WARN_ON()` | Conditional wrappers | **Not implemented** |
| Cut-here marker | `"------------[ cut here ]------------"` | **Not implemented** |
| **Status** | — | **Missing** |

### 2.5 Softlockup Detector

| Aspect | Linux | Rux |
|--------|-------|-----|
| Mechanism | Per-CPU hrtimer checks `touch_ts` | **Not implemented** |
| Threshold | 10 seconds (configurable) | — |
| Output | "BUG: soft lockup - CPU#N stuck for Xs!" + stack trace | — |
| Action | Optionally panic (`softlockup_panic`) | — |
| **Status** | — | **Missing** |

### 2.6 Hung Task Detector

| Aspect | Linux | Rux |
|--------|-------|-----|
| Mechanism | `khungtaskd` kernel thread scans all tasks | **Not implemented** |
| Detection | D-state task with unchanged `nvcsw+nivcsw` for >120s | — |
| Output | "INFO: task foo:1234 blocked for more than 120s" + stack trace | — |
| Action | Optionally panic (`hung_task_panic`) | — |
| **Status** | — | **Missing** |

### 2.7 Other DFX Features

| Feature | Linux | Rux | Priority |
|---------|-------|-----|----------|
| Hex/memory dump | `print_hex_dump()` in `lib/hexdump.c` | **Missing** | Medium |
| Magic SysRq | `drivers/tty/sysrq.c` (~600 LOC) | **Missing** | Low |
| ftrace / tracepoints | `kernel/trace/` (~10000+ LOC) | **Missing** | Low (defer) |
| Stack canary | `-fstack-protector` + `__stack_chk_fail()` | **Missing** | Low |

### 2.8 Existing Rux Debug Code Locations (to consolidate)

| Code | Current Location | Description |
|------|-----------------|-------------|
| printk + ring buffer + syslog(2) | `kernel/src/printk.rs` | **Keep as-is** — mature, 1100 LOC |
| print!/println! macros | `kernel/src/print.rs` | **Keep as-is** — thin wrapper |
| Console/UART | `kernel/src/console.rs` | **Keep as-is** — hardware driver |
| Panic handler (register/CSR/backtrace dump) | `kernel/src/main.rs:714-870` | **Move to dfx/backtrace.rs** |
| SimpleWriter (lockless UART fmt::Write) | `kernel/src/main.rs:714-726` | **Move to dfx/backtrace.rs** |
| Boot status printing | `kernel/src/main.rs:17-108` | **Keep in main.rs** (boot-specific) |
| Trap debug stubs | `kernel/src/arch/riscv64/trap.rs:509-547` | **Re-enable via dfx** |
| NMI backtrace stub | `kernel/src/interrupt/irqdesc.rs:399-408` | **Keep, integrate with dfx** |
| preempt_count context tracking | `kernel/src/interrupt/preempt.rs` | **Keep as-is** |
| Exception table fixup | `kernel/src/arch/riscv64/mm/exception.rs` | **Keep as-is** |

---

## 3. Module Design

### 3.1 Directory Structure

```
kernel/src/dfx/
├── mod.rs              — Module exports + init()
├── backtrace.rs        — dump_stack(), dump_regs(), dump_csrs(), ConsoleWriter
├── bug.rs              — BUG(), BUG_ON(), WARN(), WARN_ON(), WARN_ON_ONCE()
├── hexdump.rs          — print_hex_dump(), hex_dump_to_console()
├── softlockup.rs       — Softlockup detector (per-CPU timestamp in timer tick)
├── hung_task.rs        — Hung task detector (khungtaskd kernel thread)
└── taint.rs            — Kernel taint bitmask + taint_string()
```

### 3.2 Module Dependencies

```
taint.rs          (no deps)
hexdump.rs        (no deps)
backtrace.rs      (depends on console.rs for output)
bug.rs            (depends on backtrace.rs, taint.rs)
softlockup.rs     (depends on backtrace.rs, taint.rs, scheduler_tick)
hung_task.rs      (depends on backtrace.rs, taint.rs, kthread, task nvcsw/nivcsw)
mod.rs            (re-exports all, calls init())
```

---

## 4. Implementation Plan

### Phase 1: Foundation (taint + hexdump + backtrace)

**4.1.1 `dfx/taint.rs`**

Linux-compatible taint bitmask using `bitflags`:
- `TaintFlags` bitflag with 16 flags: `PROPRIETARY_MODULE`, `FORCED_MODULE`, `WARN`, `SOFTLOCKUP`, `DIE`, `USER`, etc.
- Global `AtomicU32` with `add_taint(flag)` and `get_taints() -> u32`
- `taint_string(buf: &mut [u8])` — convert bitmask to Linux-style `'GWF...'` character string

**4.1.2 `dfx/hexdump.rs`**

```rust
/// Print hex dump of memory region to console.
/// `prefix` is prepended to each line. Output format:
///   00000000: 7f 45 4c 46 02 01 01 00  00 00 00 00 00 00 00 00  |.ELF............|
pub fn hex_dump_to_console(addr: usize, len: usize, prefix: &str);
```

**4.1.3 `dfx/backtrace.rs`** — Extract from main.rs panic handler

```rust
/// Lockless console writer (bypasses printk, for crash/panic contexts)
pub struct ConsoleWriter { /* writes via console::putchar_no_lock */ }
impl fmt::Write for ConsoleWriter { ... }

/// Walk frame pointers, call callback for each frame (max 32 frames)
pub fn walk_stack_trace(cb: &mut dyn FnMut(u64, u64));

/// Print formatted stack trace to console
pub fn dump_stack();

/// Print register dump from inline asm capture
pub fn dump_regs_inline();

/// Print CSR state (sstatus, scause, stval, sepc)
pub fn dump_csrs();

/// Print register dump from PtRegs
pub fn dump_regs(regs: &PtRegs);
```

**4.1.4 Simplify panic handler** in `main.rs`:

Move `SimpleWriter`, register capture asm, CSR printing, and frame-pointer walk to `backtrace.rs`. Panic handler becomes ~20 lines: print message → call `dump_regs_inline()` → `dump_csrs()` → `dump_stack()` → flush → halt.

### Phase 2: BUG/WARN Macros

**4.2.1 `dfx/bug.rs`**

```rust
#[track_caller]
pub fn warn(file: &str, line: u32, condition: &str);
// Output: "------------[ cut here ]------------"
//         "WARNING: kernel/src/mm/buddy.rs:123"
//         CPU/PID/Comm header + stack trace + taint string
//         "---[ end trace ... ]---"

#[track_caller]
pub fn bug(file: &str, line: u32);
// Output: Same header format, then panic!("BUG: ...")
```

**Macros (exported from `dfx/mod.rs`):**

| Macro | Behavior |
|-------|----------|
| `warn_on!(cond)` | If true: print warning + stack trace + taint, return true. Otherwise false. |
| `warn_on_once!(cond)` | Same but only fires once per callsite (uses `AtomicBool` per location). |
| `bug_on!(cond)` | If true: print BUG + stack trace, then `panic!()`. |
| `bug!()` | Unconditional `panic!("BUG: file:line")`. |

### Phase 3: Softlockup Detector

**4.3.1 `dfx/softlockup.rs`**

```
Per-CPU data:
  touch_ts: AtomicU64   — last scheduler_tick() timestamp for this CPU

On scheduler_tick():
  softlockup::touch(cpu)    — touch_ts[cpu] = sched_clock()

On timer softirq (every ~4 seconds):
  for each cpu:
    elapsed = now - touch_ts[cpu]
    if elapsed > SOFTLOCKUP_THRESHOLD_SECS:
      pr_emerg!("BUG: soft lockup - CPU#{} stuck for {}s!", cpu, elapsed)
      dump_stack()
      add_taint(TAINT_SOFTLOCKUP)
```

**Integration points:**
- `kernel/src/sched/sched.rs` — call `dfx::softlockup::touch(cpu)` in `scheduler_tick()`
- `kernel/src/config.rs` — add `SOFTLOCKUP_THRESHOLD_SECS: u64 = 10`
- Check can run from timer softirq or a dedicated watchdog timer

### Phase 4: Hung Task Detector

**4.4.1 `dfx/hung_task.rs`**

```
Per-task tracking:
  hung_task_last_switch: AtomicU64   — nvcsw + nivcsw at last check
  hung_task_timestamp:   AtomicU64   — when last seen in D state

khungtaskd kernel thread (wakes every timeout/2 seconds):
  for_each_task():
    if task.state == UNINTERRUPTIBLE:
      switch_count = task.nvcsw + task.nivcsw
      if switch_count == hung_task_last_switch[task]:
        if now - hung_task_timestamp[task] > HUNG_TASK_TIMEOUT_SECS:
          pr_warn!("INFO: task {}:{} blocked for more than {}s",
                   comm, pid, timeout)
          dump_stack()
      else:
        hung_task_last_switch[task] = switch_count
        hung_task_timestamp[task] = now
    else:
      hung_task_timestamp[task] = now
```

**Integration points:**
- `kernel/src/process/task.rs` — add `nvcsw: AtomicU64` and `nivcsw: AtomicU64` fields
- Increment `nvcsw` on voluntary context switch (syscall, sleep)
- Increment `nivcsw` on involuntary context switch (preemption)
- `kernel/src/config.rs` — add `HUNG_TASK_TIMEOUT_SECS: u64 = 120`

### Phase 5: Module Integration

**4.5.1 `dfx/mod.rs`**

```rust
pub mod taint;
pub mod backtrace;
pub mod bug;
pub mod hexdump;
pub mod softlockup;
pub mod hung_task;

// Re-export macros
pub use bug::{warn_on, warn_on_once, bug_on, bug};

pub fn init() {
    softlockup::init();
    hung_task::init();
    pr_info!("dfx: diagnostic subsystem initialized");
}
```

**Modified files:**

| File | Change |
|------|--------|
| `kernel/src/main.rs` | Add `mod dfx;`, call `dfx::init()` after `sched::init()`, simplify panic handler |
| `kernel/src/sched/sched.rs` | Call `dfx::softlockup::touch(cpu)` in `scheduler_tick()` |
| `kernel/src/process/task.rs` | Add `nvcsw`/`nivcsw` fields, increment on context switches |
| `kernel/src/config.rs` | Add `SOFTLOCKUP_THRESHOLD_SECS`, `HUNG_TASK_TIMEOUT_SECS` |

---

## 5. Implementation Priority

| Priority | Step | Effort | Rationale |
|----------|------|--------|-----------|
| **P0** | taint.rs | Small | Foundation for WARN/softlockup |
| **P0** | hexdump.rs | Small | Useful everywhere, no deps |
| **P0** | backtrace.rs | Medium | Extract from panic, foundation for all DFX |
| **P0** | Simplify panic handler | Small | After backtrace extraction |
| **P1** | bug.rs (WARN/BUG) | Medium | Most requested DFX feature |
| **P2** | softlockup.rs | Medium | Catches spinlock deadlocks, infinite loops |
| **P2** | hung_task.rs | Medium | Catches I/O hangs, D-state tasks |
| **P3** | mod.rs + init() | Small | Integration, last step |

---

## 6. Affected Files

### New Files

| File | Lines (est.) | Content |
|------|-------------|---------|
| `kernel/src/dfx/mod.rs` | ~30 | Module exports + init() |
| `kernel/src/dfx/taint.rs` | ~80 | Taint bitmask + string conversion |
| `kernel/src/dfx/backtrace.rs` | ~200 | dump_stack(), dump_regs(), ConsoleWriter |
| `kernel/src/dfx/bug.rs` | ~100 | BUG/WARN macros + implementation |
| `kernel/src/dfx/hexdump.rs` | ~80 | Hex/memory dump utility |
| `kernel/src/dfx/softlockup.rs` | ~120 | Softlockup detector |
| `kernel/src/dfx/hung_task.rs` | ~150 | Hung task detector + khungtaskd |

### Modified Files

| File | Change Scope |
|------|-------------|
| `kernel/src/main.rs` | Add `mod dfx`, simplify panic handler (net ~80 lines removed) |
| `kernel/src/sched/sched.rs` | Add softlockup touch in scheduler_tick() (3 lines) |
| `kernel/src/process/task.rs` | Add nvcsw/nivcsw fields (10 lines) |
| `kernel/src/config.rs` | Add 2 config constants (4 lines) |

---

## 7. Verification

1. `make build` — compile without errors
2. Boot + shell: `echo -e "\n" | timeout 10 make run 2>&1 | tail -30`
3. Smoke test 6 runs: 3 in one boot + 3 after reboot
4. Verify panic output still shows registers + CSRs + backtrace
5. Verify `dump_stack()` works (add temporary test call)
6. Verify softlockup detection (create intentional busy loop, confirm detection message)
