# Interrupt Subsystem Refactoring Plan

## 1. Current State Overview

### 1.1 Implemented Modules in Rux

| Module | File | Functionality |
|--------|------|---------------|
| Trap entry/exit | `kernel/src/arch/riscv64/trap.S` | Assembly register save/restore, user/kernel detection, signal+resched loop |
| Trap dispatch | `kernel/src/arch/riscv64/trap.rs` | scause dispatch: timer/IPI/external/exception/syscall |
| PtRegs | `kernel/src/arch/riscv64/pt_regs.rs` | 288-byte register frame, Cause enum, CSR bit definitions |
| PLIC driver | `kernel/src/drivers/intc/plic.rs` | claim/complete/enable, static singleton |
| CLINT/SBI | `kernel/src/drivers/intc/clint.rs` | SBI timer + IPI |
| Timer | `kernel/src/drivers/timer/riscv64.rs` | jiffies, stimecmp, 10ms period |
| IPI | `kernel/src/arch/riscv64/ipi.rs` | Reschedule/Stop IPI types, SBI send |
| Interrupt stack | `kernel/src/arch/riscv64/smp.rs` | 16KB per-CPU interrupt stack |
| Context switch | `kernel/src/arch/riscv64/context.rs` | `__switch_to` assembly, switch_mm |
| Signal | `kernel/src/signal.rs` | Only deferred execution mechanism |
| Kernel big lock | `kernel/src/sync/kernel_lock.rs` | `AtomicU64` global spinlock |
| Interrupt stats | `kernel/src/fs/procfs/interrupts.rs` | `/proc/interrupts` per-CPU counters |

### 1.2 Linux RISC-V Complete Architecture (Reference)

```
Hardware Interrupt
  → entry.S: handle_exception (register save + IRQ stack switch)
    → traps.c: do_irq()
      → irqentry_enter()           // RCU/lockdep/preempt count
      → handle_riscv_irq()
        → irq_enter_rcu()
        → riscv_intc_irq()         // INTC root controller
          → generic_handle_domain_irq()
            → plic_handle_irq()    // PLIC chained handler
              → flow handler (fasteoi/edge/level)
                → action->handler()   // Device driver (top half)
                → __irq_wake_thread() // Wake interrupt thread (optional)
        → irq_exit_rcu()
          → invoke_softirq()       // Bottom half processing
      → irqentry_exit()            // Preemption check/resched
    → ret_from_exception: sret
```

---

## 2. Module-by-Module Difference Analysis

### 2.1 IRQ Framework Core (irqdesc / irq_chip / irq_domain)

**Linux Implementation:**

| Component | File | Functionality |
|-----------|------|---------------|
| `struct irq_desc` | `kernel/irq/irqdesc.c` | IRQ descriptor: state, lock, action list, thread count |
| `struct irq_chip` | `include/linux/irq.h` | HW operations: mask/unmask/ack/eoi/set_type/set_affinity |
| `struct irq_domain` | `kernel/irq/irqdomain.c` | Hardware IRQ → Linux IRQ number mapping |
| `struct irqaction` | `include/linux/interrupt.h` | Handler function list: handler + thread_fn + flags |
| Flow handlers | `kernel/irq/chip.c` | `handle_fasteoi_irq`, `handle_edge_irq`, `handle_level_irq`, etc. |
| `request_irq()` | `kernel/irq/manage.c` | Register interrupt handler |
| `free_irq()` | `kernel/irq/manage.c` | Unregister interrupt handler |

**Rux Status:** None. Interrupt handling is hardcoded in `trap.rs` `handle_external_interrupt()`:

```rust
// trap.rs:261-293 - Hardcoded IRQ dispatch
match irq {
    1..=8 => virtio::interrupt_handler(),
    32..=127 => virtio::interrupt_handler_pci(irq),
    10 => { /* UART: no-op */ }
    11..=13 => ipi::handle_ipi(irq, hart),
    _ => {}
}
plic::complete(hart, irq);
```

**Difference Summary:**

| Feature | Linux | Rux |
|---------|-------|-----|
| IRQ descriptor abstraction | `irq_desc` + radix tree | None |
| irq_chip abstraction | mask/unmask/ack/eoi | Direct PLIC MMIO |
| IRQ domain mapping | hwirq → Linux IRQ | No mapping, uses hardware numbers directly |
| Interrupt registration API | `request_irq()` / `free_irq()` | None, drivers call directly |
| Shared interrupts | action linked list | Not supported |
| IRQ flow handler | fasteoi/edge/level/percpu | No distinction, uniform handling |
| Interrupt disable nesting | `depth` count | None |
| /proc/interrupts | Dynamically generated from irq_desc | Hardcoded counters |

### 2.2 Interrupt Stack

**Linux Implementation:**

| Feature | Implementation |
|---------|----------------|
| Stack size | `THREAD_SIZE` (typically 16KB) |
| Allocation | vmalloc when `CONFIG_VMAP_STACK`, otherwise static per-CPU |
| Switch logic | `call_on_irq_stack()` assembly, only when `on_thread_stack()` |
| Overflow detection | VMAP stack + guard page, 4KB overflow stack |
| Shadow Call Stack | `CONFIG_SHADOW_CALL_STACK` support |
| Softirq stack | `do_softirq_own_stack()` reuses IRQ stack |

**Rux Status (`smp.rs`):**

| Feature | Implementation |
|---------|----------------|
| Stack size | 16KB per-CPU (`INTR_STACK_SIZE = 16384`) |
| Allocation | Static BSS array `PER_CPU_INTR_STACKS[4]` |
| Switch logic | `GET_PER_CPU_INTR_STACK` in `trap.S`, sp range check |
| Overflow detection | None |
| Shadow Call Stack | None |

**Differences:**

| Feature | Linux | Rux |
|---------|-------|-----|
| Stack size | Configurable, equals THREAD_SIZE | Fixed 16KB |
| VMAP stack | Supported | Not supported |
| Overflow detection | guard page + overflow stack | None |
| on_thread_stack() | Yes, precise detection | sp range approximation |
| Softirq reuse | `do_softirq_own_stack()` | No softirq |

### 2.3 Top Half

**Linux Implementation:**

- Standardized top half via `irq_chip` + flow handler
- `handle_irq_event()` → iterates `action` list calling `handler()`
- Handler returns `IRQ_HANDLED` / `IRQ_WAKE_THREAD`
- Supports `IRQF_SHARED`, `IRQF_ONESHOT`, `IRQF_PERCPU` flags
- Interrupt nesting via `IRQF_DISABLED` (deprecated, now non-nested by default)

**Rux Status:**

- All interrupts processed synchronously in `trap_handler()`
- No standardized handler registration/return mechanism
- No shared interrupt support
- No `IRQ_HANDLED` / `IRQ_WAKE_THREAD` semantics

### 2.4 Bottom Half (Softirq / Tasklet / Workqueue)

**Linux Implementation (Four-Layer Architecture):**

| Layer | Mechanism | Context | Can Sleep | File |
|-------|-----------|---------|-----------|------|
| 1 | Softirq | Interrupt context | No | `kernel/softirq.c` |
| 2 | Tasklet | Interrupt context | No | `kernel/softirq.c` |
| 3 | Workqueue | Process context | Yes | `kernel/workqueue.c` |
| 4 | Threaded IRQ | Process context | Yes | `kernel/irq/manage.c` |

**Softirq Vectors:**
```c
HI_SOFTIRQ(0), TIMER_SOFTIRQ, NET_TX_SOFTIRQ, NET_RX_SOFTIRQ,
BLOCK_SOFTIRQ, IRQ_POLL_SOFTIRQ, TASKLET_SOFTIRQ, SCHED_SOFTIRQ,
HRTIMER_SOFTIRQ, RCU_SOFTIRQ
```

**ksoftirqd:** Per-CPU kernel thread processing backlogged softirqs (max 10 loops or 2ms)

**Rux Status:** No bottom half mechanism at all. All work completes synchronously in the top half.

**Impact:**
- VirtIO block device interrupt includes I/O completion callback (wakes waiting process)
- Network interrupt directly calls `ethernet_poll()` for packet reception
- TCP timer iterates all sockets directly in timer interrupt
- Work that should run in bottom half blocks interrupt processing

### 2.5 Threaded Interrupts

**Linux Implementation (`kernel/irq/manage.c`):**

```
request_threaded_irq(irq, handler, thread_fn, flags, name, dev_id)
  → handler (hardirq context): Quick acknowledge, returns IRQ_WAKE_THREAD
  → thread_fn (process context): Actual work, can sleep
  → Kernel thread "irq/%d-%s", SCHED_FIFO scheduling
  → IRQF_ONESHOT: Keep IRQ masked until thread completes
  → Forced threading: threadirqs boot param forces all interrupts threaded
```

**Rux Status:** None. All interrupt processing completes synchronously in hard interrupt context.

### 2.6 Non-Maskable Interrupts (NMI)

**Linux Implementation:**

- `request_nmi()` / `request_percpu_nmi()` for registration
- `handle_fasteoi_nmi()` flow handler: lock-free, single action
- On RISC-V: kernel-mode exceptions use NMI-like entry path (`irqentry_nmi_enter/exit`)
- `arch_trigger_cpumask_backtrace()` for NMI backtrace
- `IRQCHIP_SUPPORTS_NMI` flag

**Rux Status:** No NMI support whatsoever.

### 2.7 preempt_count and Interrupt Context Detection

**Linux Implementation:**

```c
preempt_count layout (32-bit):
  [0:7]   PREEMPT_MASK      - Preemption disable count
  [8:15]  SOFTIRQ_MASK      - Softirq count
  [16:19] HARDIRQ_MASK      - Hardirq count
  [20]    NMI_MASK           - NMI count
  [26]    PREEMPT_ACTIVE     - Actively preempting
```

- `in_interrupt()`: hardirq + softirq + NMI any non-zero
- `in_task()`: `!in_interrupt() && !in_irq()`
- `in_irq()`: hardirq non-zero
- `in_softirq()`: softirq non-zero

**Rux Status (`pt_regs.rs:365`):**

```rust
pub fn in_interrupt() -> bool {
    // TODO: Implement preemption count check
    false
}
```

`ti_preempt_count` field exists in `Task` struct but is never used.

### 2.8 PLIC Driver

**Linux Implementation (`irq-sifive-plic.c`):**

| Feature | Implementation |
|---------|----------------|
| Data structure | `plic_priv` (global) + `plic_handler` (per-CPU) |
| IRQ chip | `plic_chip` (level) + `plic_edge_chip` (edge) |
| Interrupt affinity | `plic_set_affinity()`: cross-hart migration |
| Interrupt priority | Full 0-7 priority support |
| Per-CPU lock | `enable_lock` protects enable registers |
| Domain registration | `irq_domain_create_tree()` |
| Claim loop | `do {} while` loop processes all pending IRQs |
| Suspend save | `prio_save` / `enable_save` for suspend/resume |

**Rux Status (`plic.rs`):**

| Feature | Implementation |
|---------|----------------|
| Data structure | Simple `Plic { base, num_harts }` |
| IRQ chip | No abstraction |
| Interrupt affinity | Not supported |
| Interrupt priority | Fixed priority 1 |
| Per-CPU lock | None |
| Domain registration | None |
| Claim loop | Single claim (no loop) |
| Suspend save | None |

### 2.9 Timer Interrupt

**Linux Implementation:**

| Feature | Implementation |
|---------|----------------|
| Framework | `clock_event_device` + `clocksource` |
| Registration | `request_percpu_irq()` for per-CPU timer handler |
| Scheduler tick | `tick_device` → `scheduler_tick()` |
| High-res timers | `hrtimer` framework, `HRTIMER_SOFTIRQ` |
| NO_HZ | Tickless idle support |

**Rux Status (`timer/riscv64.rs`):**

| Feature | Implementation |
|---------|----------------|
| Framework | Direct `stimecmp` CSR write |
| Registration | Hardcoded in `trap.rs` match branch |
| Scheduler tick | **TODO: `scheduler_tick()` not called** |
| High-res timers | None |
| NO_HZ | None |

**Critical Defect:** `scheduler_tick()` is commented out in `handle_timer_interrupt()`, time-slice preemption is non-functional.

### 2.10 IPI Subsystem

**Linux Implementation:**

| Feature | Implementation |
|---------|----------------|
| IPI multiplex | `sbi-ipi.c`: multiple IPI types over single soft interrupt |
| IPI types | Reschedule, Call function, CPU stop, IRQ work, timer, etc. |
| IPI handling | `ipi_mux_create()` → per-type handler |
| Cross-CPU call | `smp_call_function()` family |

**Rux Status (`ipi.rs`):**

| Feature | Implementation |
|---------|----------------|
| IPI types | Only Reschedule + Stop |
| Send method | SBI `send_ipi` + PLIC trigger |
| Handling | Direct `schedule()` |

### 2.11 UART Interrupt-Driven I/O

**Linux:** UART uses interrupt-driven RX/TX, `request_irq()` registers handlers.

**Rux:** UART IRQ 10 is enabled in PLIC but handler is empty (`trap.rs:280`), RX/TX relies entirely on polling.

### 2.12 Kernel Big Lock vs Fine-Grained Locking

**Linux:** No global kernel lock. Each subsystem has its own locks (`rq->lock`, `irq_desc->lock`, `mm->mmap_lock`, etc.).

**Rux:** `KERNEL_LOCK` (global `AtomicU64` spinlock) acquired on user→kernel transition, released on return to user. All kernel code serializes under this single lock.

---

## 3. Refactoring Plan

### Phase 1: IRQ Framework Core (Priority: High)

> Goal: Build Linux-compatible IRQ registration/dispatch infrastructure

#### 1.1 irq_desc / irq_chip / irq_domain

**New files:** `kernel/src/interrupt/`

| File | Content |
|------|---------|
| `mod.rs` | Module exports |
| `irqdesc.rs` | `IrqDesc` struct: state lock, action list, irq_data, depth |
| `irqchip.rs` | `IrqChip` trait: mask/unmask/ack/eoi/set_affinity |
| `irqdomain.rs` | `IrqDomain`: hwirq → Linux IRQ mapping, linear/radix tree |
| `handle.rs` | Flow handlers: `handle_fasteoi_irq`, `handle_edge_irq`, `handle_level_irq` |
| `manage.rs` | `request_irq()`, `free_irq()`, `request_threaded_irq()` |
| `spurious.rs` | Spurious interrupt detection |

**IrqChip trait:**
```rust
pub trait IrqChip {
    fn irq_enable(&self, data: &mut IrqData);
    fn irq_disable(&self, data: &mut IrqData);
    fn irq_ack(&self, data: &mut IrqData);
    fn irq_mask(&self, data: &mut IrqData);
    fn irq_unmask(&self, data: &mut IrqData);
    fn irq_eoi(&self, data: &mut IrqData);
    fn irq_set_affinity(&self, data: &mut IrqData, cpu: usize);
    fn irq_set_type(&self, data: &mut IrqData, flow_type: u32);
    fn irq_set_wake(&self, data: &mut IrqData, on: bool);
}
```

**IrqDesc struct:**
```rust
pub struct IrqDesc {
    pub irq_data: IrqData,             // chip + hwirq + Linux IRQ
    pub handle_irq: FlowHandler,       // flow handler function pointer
    pub action: Option<Box<IrqAction>>, // handler function list
    pub lock: SpinLock<()>,
    pub depth: AtomicU32,              // disable nesting count
    pub istate: AtomicU32,             // internal state
    pub name: &'static str,
}
```

**IrqAction struct:**
```rust
pub struct IrqAction {
    pub handler: fn(u32, *mut c_void) -> IrqReturn,  // top half
    pub thread_fn: Option<fn(u32, *mut c_void) -> IrqReturn>, // bottom half thread
    pub dev_id: *mut c_void,
    pub name: &'static str,
    pub irq: u32,
    pub flags: IrqFlags,
    pub next: Option<Box<IrqAction>>,  // shared interrupt list
}
```

#### 1.2 PLIC Refactored as irq_chip

**Modified file:** `kernel/src/drivers/intc/plic.rs`

| Change | Description |
|--------|-------------|
| Implement `IrqChip` trait | mask/unmask/ack/eoi/set_affinity/set_type |
| Create `IrqDomain` | `plic_domain`: hwirq → Linux IRQ linear mapping |
| Per-CPU handler | `plic_handler` struct with `enable_lock` |
| Claim loop | `do-while` loop processes all pending IRQs |
| Interrupt affinity | Cross-hart enable bit migration |
| Edge/Level distinction | Dual chip registration (`plic_chip` + `plic_edge_chip`) |

#### 1.3 PLIC as Chained Controller

PLIC registers on INTC (RISC-V core-local) `RV_IRQ_EXT`:

```rust
// INTC domain: scause cause → local IRQ dispatch
// PLIC domain: claim → hwirq → flow handler → action chain
let plic_domain = IrqDomain::new_linear(plic_chip, MAX_IRQS);
irq_set_chained_handler(RV_IRQ_EXT, plic_handle_irq);
```

### Phase 2: Interrupt Stack Enhancement (Priority: Medium)

**Modified files:** `kernel/src/arch/riscv64/smp.rs`, `kernel/src/arch/riscv64/trap.S`

| Change | Description |
|--------|-------------|
| Stack size aligned to THREAD_SIZE | 16KB → unified with `KERNEL_STACK_SIZE` |
| Overflow detection | Add guard page + 4KB overflow stack |
| `on_thread_stack()` | Precise detection of whether current sp is on thread stack |
| Softirq stack reuse | `do_softirq_own_stack()` reuses IRQ stack |

### Phase 3: Bottom Half — Softirq + Tasklet (Priority: High)

> Goal: Move time-consuming work out of hard interrupt context

#### 3.1 Softirq Framework

**New file:** `kernel/src/interrupt/softirq.rs`

| Component | Description |
|-----------|-------------|
| `SoftirqAction` | `struct { action: fn() }` |
| `softirq_vec[10]` | Global softirq vector array |
| `raise_softirq(nr)` | Mark pending + wake ksoftirqd |
| `__do_softirq()` | Process pending softirqs (max 10 rounds or 2ms) |
| `invoke_softirq()` | Called at `irq_exit()` time |

**Softirq Vectors (aligned with Linux):**
```rust
pub enum SoftirqIndex {
    HI = 0, TIMER, NET_TX, NET_RX, BLOCK,
    IRQ_POLL, TASKLET, SCHED, HRTIMER, RCU,
}
```

#### 3.2 Tasklet

**New file:** `kernel/src/interrupt/tasklet.rs`

| Component | Description |
|-----------|-------------|
| `TaskletStruct` | state, callback, data |
| `tasklet_schedule()` | Queue to per-CPU TASKLET_SOFTIRQ |
| `tasklet_hi_schedule()` | Queue to per-CPU HI_SOFTIRQ |
| `tasklet_action()` | Process normal tasklet queue |
| `tasklet_hi_action()` | Process high-priority tasklet queue |

#### 3.3 ksoftirqd

**New file:** `kernel/src/interrupt/ksoftirqd.rs`

| Component | Description |
|-----------|-------------|
| Per-CPU kernel thread | `ksoftirqd/%u` |
| Wake condition | softirq loop exceeds MAX_SOFTIRQ_RESTART |
| Scheduling policy | SCHED_OTHER, low priority |

#### 3.4 Migrate Existing Drivers

| Driver | Current (in top half) | After Migration (bottom half) |
|--------|----------------------|-------------------------------|
| VirtIO Block | `interrupt_handler()` completes I/O | Top half ack only, completion in `BLOCK_SOFTIRQ` |
| VirtIO Net | `interrupt_handler()` calls `ethernet_poll()` | Top half ack only, poll in `NET_RX_SOFTIRQ` |
| TCP Timer | Timer interrupt iterates all sockets | Process in `TIMER_SOFTIRQ` |
| UART | No-op (polling) | Top half receives chars, process in `TASKLET_SOFTIRQ` |

### Phase 4: Threaded Interrupts (Priority: Medium)

> Goal: Support `request_threaded_irq()`, allow interrupt handling in process context

**New file:** `kernel/src/interrupt/thread.rs`

| Component | Description |
|-----------|-------------|
| `irq_thread()` | Interrupt thread main loop, waits for `IRQTF_RUNTHREAD` |
| `setup_irq_thread()` | Create `irq/%d-%s` kernel thread |
| `__irq_wake_thread()` | Top half wakes interrupt thread |
| `irq_finalize_oneshot()` | Unmask on `IRQF_ONESHOT` completion |
| Forced threading | `threadirqs` boot parameter support |

**Thread creation flow:**
```
request_threaded_irq(irq, handler, thread_fn, ...)
  → __setup_irq()
    → if thread_fn exists:
      → setup_irq_thread(new, irq, false)  // create "irq/%d-name" thread
    → if forced threading:
      → original handler becomes thread_fn
      → irq_default_primary_handler becomes handler
```

**Thread wake flow:**
```
hardirq handler → returns IRQ_WAKE_THREAD
  → __irq_wake_thread()
    → set IRQTF_RUNTHREAD
    → wake_up_process(action->thread)
  → irq_thread()
    → action->thread_fn(irq, dev_id)
    → if IRQF_ONESHOT: irq_finalize_oneshot()
```

### Phase 5: preempt_count Implementation (Priority: High)

> Goal: Correctly track interrupt/softirq/preemption context

**Modified files:** `kernel/src/process/task.rs`, `kernel/src/arch/riscv64/trap.rs`, `kernel/src/arch/riscv64/trap.S`

**preempt_count layout (Linux-compatible):**
```rust
// ti_preempt_count layout (32-bit):
const PREEMPT_MASK: i32   = 0x000000FF;  // bits [0:7]
const SOFTIRQ_MASK: i32   = 0x0000FF00;  // bits [8:15]
const HARDIRQ_MASK: i32   = 0x000F0000;  // bits [16:19]
const NMI_MASK: i32       = 0x00100000;  // bit [20]
const PREEMPT_ACTIVE: i32 = 0x04000000;  // bit [26]
```

**Entry/exit point modifications:**

| Location | Operation |
|----------|-----------|
| `irqentry_enter()` | `preempt_count += HARDIRQ_OFFSET` (bit 16) |
| `irqentry_exit()` | `preempt_count -= HARDIRQ_OFFSET` |
| Softirq entry | `preempt_count += SOFTIRQ_OFFSET` (bit 8) |
| Softirq exit | `preempt_count -= SOFTIRQ_OFFSET` |
| `__do_softirq()` | Check `!in_interrupt()` to decide whether to wake ksoftirqd |

**API implementation:**
```rust
pub fn in_interrupt() -> bool { (preempt_count & (HARDIRQ_MASK | SOFTIRQ_MASK | NMI_MASK)) != 0 }
pub fn in_irq() -> bool { (preempt_count & HARDIRQ_MASK) != 0 }
pub fn in_softirq() -> bool { (preempt_count & SOFTIRQ_MASK) != 0 }
pub fn in_task() -> bool { !in_interrupt() }
```

### Phase 6: Timer Interrupt Fix (Priority: High)

> Goal: Enable scheduler tick, fix time-slice preemption

**Modified files:** `kernel/src/arch/riscv64/trap.rs`, `kernel/src/drivers/timer/riscv64.rs`

| Change | Description |
|--------|-------------|
| Uncomment `scheduler_tick()` | Call in `handle_timer_interrupt()` |
| Register as per-CPU IRQ | `request_percpu_irq(RV_IRQ_TIMER, timer_handler)` |
| Add `SCHED_SOFTIRQ` | `scheduler_tick()` triggers softirq for load balancing |
| Process time accounting | Update `utime`/`stime`, `account_process_tick()` |

### Phase 7: IPI Enhancement (Priority: Low)

> Goal: Support more IPI types, multiplex over soft interrupt

**Modified file:** `kernel/src/arch/riscv64/ipi.rs`

| Change | Description |
|--------|-------------|
| IPI type expansion | Reschedule, Call function, CPU stop, IRQ work |
| IPI multiplex | Single SBI IPI carries bitmap, send multiple types at once |
| Call function | `smp_call_function()` cross-CPU function call |
| IPI handler registration | Via `irq_domain`, no longer hardcoded |

### Phase 8: UART Interrupt-Driven I/O (Priority: Low)

**Modified files:** `kernel/src/console.rs`, `kernel/src/drivers/intc/plic.rs`

| Change | Description |
|--------|-------------|
| RX interrupt handling | IRQ 10 handler reads receive FIFO |
| Input buffer | Ring buffer to store received characters |
| Wake waiting process | `wait_queue` wakes processes blocked on `read()` |
| Remove polling | `getchar()` reads from buffer + blocking wait |

### Phase 9: NMI Support (Priority: Low)

> RISC-V base ISA has no hardware NMI, but framework can be established

**New file:** `kernel/src/interrupt/nmi.rs`

| Component | Description |
|-----------|-------------|
| `request_nmi()` | NMI handler registration |
| `handle_fasteoi_nmi()` | NMI flow handler (lock-free) |
| `irqentry_nmi_enter/exit()` | NMI entry/exit paths |
| NMI backtrace | `arch_trigger_cpumask_backtrace()` |

---

## 4. Implementation Priority and Dependencies

```
Phase 1 (IRQ Framework) ──┬──→ Phase 3 (Softirq/Tasklet) ──→ Phase 4 (Threaded)
                           │
Phase 5 (preempt_count) ──┤
                           │
Phase 6 (Timer Fix) ──────┤
                           │
Phase 2 (IRQ Stack) ──────┼──→ Phase 7 (IPI Enhancement)
                           │
                           └──→ Phase 8 (UART Interrupt) ──→ Phase 9 (NMI)
```

### Recommended Implementation Order

| Stage | Content | Effort | Dependencies |
|-------|---------|--------|--------------|
| **Phase 5** | preempt_count | Small | None, independent |
| **Phase 6** | Timer fix | Small | None, independent |
| **Phase 1** | IRQ framework core | Large | None |
| **Phase 2** | Interrupt stack enhancement | Medium | None |
| **Phase 3** | Softirq/Tasklet | Large | Depends on Phase 1 + 5 |
| **Phase 4** | Threaded interrupts | Large | Depends on Phase 3 |
| **Phase 7** | IPI enhancement | Medium | Depends on Phase 1 |
| **Phase 8** | UART interrupt-driven | Medium | Depends on Phase 1 |
| **Phase 9** | NMI support | Small | Depends on Phase 1 |

**Recommended: Phase 5 + 6 first (quick fixes), then Phase 1 (core refactoring), then Phase 2-4 (bottom half + threading), finally Phase 7-9 (polish).**

---

## 5. Kernel Big Lock Exit Path

The current `KERNEL_LOCK` is the root cause of all kernel code serialization. Interrupt subsystem refactoring creates conditions for big lock removal:

| Stage | Replacement Lock |
|-------|-----------------|
| Phase 1 complete | `irq_desc->lock` replaces global lock for IRQ operations |
| Phase 3 complete | softirq needs no big lock (per-CPU data) |
| Phase 5 complete | preempt_count enables safe preemption |
| Final | `rq->lock`, `irq_desc->lock`, `mm->mmap_lock` etc. fully replace big lock |

---

## 6. Affected Files

### New Files

| File | Phase |
|------|-------|
| `kernel/src/interrupt/mod.rs` | 1 |
| `kernel/src/interrupt/irqdesc.rs` | 1 |
| `kernel/src/interrupt/irqchip.rs` | 1 |
| `kernel/src/interrupt/irqdomain.rs` | 1 |
| `kernel/src/interrupt/handle.rs` | 1 |
| `kernel/src/interrupt/manage.rs` | 1 |
| `kernel/src/interrupt/spurious.rs` | 1 |
| `kernel/src/interrupt/softirq.rs` | 3 |
| `kernel/src/interrupt/tasklet.rs` | 3 |
| `kernel/src/interrupt/ksoftirqd.rs` | 3 |
| `kernel/src/interrupt/thread.rs` | 4 |
| `kernel/src/interrupt/nmi.rs` | 9 |

### Modified Files

| File | Phase | Change Scope |
|------|-------|-------------|
| `kernel/src/drivers/intc/plic.rs` | 1 | Rewrite as IrqChip implementation |
| `kernel/src/arch/riscv64/trap.rs` | 1,5,6 | IRQ dispatch via domain lookup, preempt_count adjustments, scheduler_tick |
| `kernel/src/arch/riscv64/trap.S` | 2,5 | Interrupt stack switch optimization, preempt_count assembly interface |
| `kernel/src/arch/riscv64/pt_regs.rs` | 5 | Remove `in_interrupt()` stub |
| `kernel/src/arch/riscv64/smp.rs` | 2 | Add overflow detection to interrupt stack |
| `kernel/src/drivers/timer/riscv64.rs` | 6 | Timer handler registered via IRQ framework |
| `kernel/src/arch/riscv64/ipi.rs` | 7 | IPI type expansion + multiplex |
| `kernel/src/console.rs` | 8 | UART interrupt-driven RX/TX |
| `kernel/src/process/task.rs` | 5 | `ti_preempt_count` actual usage |
| `kernel/src/sched/sched.rs` | 5,6 | preempt_count checks, scheduler_tick integration |
| `kernel/src/drivers/virtio/mod.rs` | 3 | Interrupt handling migrated to softirq |
| `kernel/src/drivers/net/virtio_net.rs` | 3 | Interrupt handling migrated to softirq |
| `kernel/src/net/tcp_timer.rs` | 3 | Migrated to TIMER_SOFTIRQ |
