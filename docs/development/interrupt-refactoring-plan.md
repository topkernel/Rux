# Interrupt Subsystem Refactoring Plan

## 1. Current State Overview

### 1.1 Implemented Modules in Rux

| Module | File | Functionality |
|--------|------|---------------|
| **IRQ Framework** | `kernel/src/interrupt/` | irq_desc, irq_chip, irq_domain, request_irq/free_irq, flow handler |
| PLIC as IrqChip | `kernel/src/drivers/intc/plic.rs` | PLIC implements IrqChip + IrqDomainOps, creates linear irq_domain |
| Trap entry/exit | `kernel/src/arch/riscv64/trap.S` | Assembly register save/restore, user/kernel detection, signal+resched loop |
| Trap dispatch | `kernel/src/arch/riscv64/trap.rs` | scause dispatch → PLIC claim → generic_handle_domain_irq() |
| PtRegs | `kernel/src/arch/riscv64/pt_regs.rs` | 288-byte register frame, Cause enum, CSR bit definitions |
| CLINT/SBI | `kernel/src/drivers/intc/clint.rs` | SBI timer + IPI |
| Timer | `kernel/src/drivers/timer/riscv64.rs` | jiffies, stimecmp, 10ms period |
| IPI | `kernel/src/arch/riscv64/ipi.rs` | Reschedule/Stop IPI types, SBI send, registered via request_irq |
| Interrupt stack | `kernel/src/arch/riscv64/smp.rs` | 16KB per-CPU interrupt stack |
| Context switch | `kernel/src/arch/riscv64/context.rs` | `__switch_to` assembly, switch_mm |
| Signal | `kernel/src/signal.rs` | Only deferred execution mechanism |
| Kernel big lock | `kernel/src/sync/kernel_lock.rs` | `AtomicU64` global spinlock |
| Interrupt stats | `kernel/src/fs/procfs/interrupts.rs` | `/proc/interrupts` per-CPU counters (reads from irq_desc) |

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

**Rux Status (Phase 1 ✅ COMPLETED):**

| Component | File | Functionality |
|-----------|------|---------------|
| `IrqDesc` (static array) | `kernel/src/interrupt/irqdesc.rs` | 128-entry descriptor array with action list, per-CPU stats |
| `IrqChip` (function pointer table) | `kernel/src/interrupt/irqchip.rs` | mask/unmask/ack/eoi/set_type/set_affinity ops |
| `IrqDomain` (linear) | `kernel/src/interrupt/domain.rs` | hwirq → virq identity mapping, revmap lookup |
| `IrqAction` (linked list) | `kernel/src/interrupt/irqdesc.rs` | Shared interrupt support via action chain with IRQF_SHARED |
| `handle_fasteoi_irq()` | `kernel/src/interrupt/irqdesc.rs` | Flow handler: stats → dispatch → EOI via chip |
| `request_irq()` | `kernel/src/interrupt/irqdesc.rs` | Register handler, supports shared interrupts |
| `free_irq()` | `kernel/src/interrupt/irqdesc.rs` | Unregister handler from action chain |
| PLIC IrqChip impl | `kernel/src/drivers/intc/plic.rs` | `PLIC_CHIP` with plic_mask/plic_unmask/plic_eoi |
| PLIC IrqDomain | `kernel/src/drivers/intc/plic.rs` | Linear domain, pre-maps all 128 IRQs |

**Remaining Differences:**

| Feature | Linux | Rux (after Phase 1) |
|---------|-------|-----|
| irq_desc storage | Radix tree + static array | Static array only (sufficient for PLIC) |
| IrqChip abstraction | Rust trait | Function pointer table (Rux convention) |
| IRQ domain mapping | Linear/radix/tree | Linear identity mapping only |
| Shared interrupts | ✅ action linked list | ✅ Implemented |
| IRQ flow handler | fasteoi/edge/level/percpu | fasteoi only (PLIC is level-triggered) |
| Interrupt disable nesting | `depth` count | `depth` field exists, not yet used |
| Threaded interrupts | `thread_fn` in irqaction | Not yet (Phase 4) |
| /proc/interrupts | Dynamically from irq_desc | ✅ Reads from irq_desc per-CPU counters |

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

**Rux Status (Phase 5 ✅ COMPLETED):**

Implemented in `kernel/src/interrupt/preempt.rs`:

| API | Implementation |
|-----|---------------|
| `in_interrupt()` | `(preempt_count & (HARDIRQ_MASK \| SOFTIRQ_MASK \| NMI_MASK)) != 0` |
| `in_irq()` | `(preempt_count & HARDIRQ_MASK) != 0` |
| `in_softirq()` | `(preempt_count & SOFTIRQ_MASK) != 0` |
| `in_task()` | `!in_interrupt()` |
| `preemptible()` | `preempt_count == 0` |
| `irq_enter()` | `preempt_count += HARDIRQ_OFFSET` (called in trap.rs for timer/soft/external) |
| `irq_exit()` | `preempt_count -= HARDIRQ_OFFSET` |
| Assembly check | `trap.S: .Lcheck_signals_and_resched` checks `ti_preempt_count != 0` before `asm_schedule` |

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

### Phase 1: IRQ Framework Core ✅ COMPLETED

> Goal: Build Linux-compatible IRQ registration/dispatch infrastructure
>
> **Status:** Implemented in commit `a321032`. All drivers migrated to use `request_irq()`.

#### What Was Implemented

**New files:** `kernel/src/interrupt/`

| File | Lines | Content |
|------|-------|---------|
| `mod.rs` | ~30 | Module exports, `init()` |
| `irqdesc.rs` | ~340 | `IrqDesc`, `IrqAction`, `IrqData`, `IrqReturn`, `request_irq`, `free_irq`, `handle_irq_event`, `handle_fasteoi_irq`, per-CPU stats |
| `irqchip.rs` | ~35 | `IrqChip` function pointer table (matches Rux's `BlockDeviceOps` pattern) |
| `domain.rs` | ~140 | `IrqDomain`, `IrqDomainOps`, `irq_domain_create_linear`, `generic_handle_domain_irq`, identity hwirq→virq mapping |

**Modified files:**

| File | Change |
|------|--------|
| `kernel/src/main.rs` | Added `mod interrupt;` + `interrupt::init()` in boot sequence |
| `kernel/src/arch/riscv64/trap.rs` | `handle_external_interrupt()` → PLIC claim → `generic_handle_domain_irq()`, removed hardcoded match dispatch |
| `kernel/src/drivers/intc/plic.rs` | Added `PLIC_CHIP` IrqChip, `PLIC_DOMAIN_OPS`, creates linear domain, pre-maps all 128 IRQs |
| `kernel/src/drivers/virtio/mod.rs` | Handlers use new `(irq, dev_id) -> IrqReturn` signature, `enable_device_interrupt()` uses `request_irq()` |
| `kernel/src/drivers/virtio/virtio_pci.rs` | `enable_device_interrupt()` uses `request_irq()` |
| `kernel/src/drivers/net/virtio_net.rs` | Handler signature + `request_irq()` registration |
| `kernel/src/arch/riscv64/ipi.rs` | Added `ipi_irq_handler`, `register_irq_handlers()` via `request_irq()` |
| `kernel/src/fs/procfs/interrupts.rs` | Removed 4×128 PLIC_COUNT arrays, reads from `irq_desc.per_cpu_count` |

**Key design decisions:**
- Function pointer tables (not Rust traits) to match Rux conventions
- Static `[IrqDesc; 128]` array (sufficient for PLIC's 128 IRQs)
- Identity hwirq-to-virq mapping (Phase 1 simplicity)
- PLIC EOI done by flow handler (`handle_fasteoi_irq` → `chip.irq_eoi`), not individual drivers
- Shared interrupt support via `IrqAction` linked list with `IRQF_SHARED`

### Phase 2: Interrupt Stack Enhancement (Priority: Medium) — NEXT

**Modified files:** `kernel/src/arch/riscv64/smp.rs`, `kernel/src/arch/riscv64/trap.S`

| Change | Description |
|--------|-------------|
| Stack size aligned to THREAD_SIZE | 16KB → unified with `KERNEL_STACK_SIZE` |
| Overflow detection | Add guard page + 4KB overflow stack |
| `on_thread_stack()` | Precise detection of whether current sp is on thread stack |
| Softirq stack reuse | `do_softirq_own_stack()` reuses IRQ stack |

### Phase 3: Bottom Half — Softirq + Tasklet (Priority: High) — NEXT

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

### Phase 4: Threaded Interrupts (Priority: Medium) — TODO

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

### Phase 5: preempt_count Implementation (Priority: High) — ✅ COMPLETED

> Goal: Correctly track interrupt/softirq/preemption context
>
> **Status:** Implemented. All context query APIs working, `irq_enter()`/`irq_exit()` wired into trap handler.

#### What Was Implemented

**New file:** `kernel/src/interrupt/preempt.rs`

| Component | Description |
|-----------|-------------|
| Bit mask constants | `PREEMPT_MASK`, `SOFTIRQ_MASK`, `HARDIRQ_MASK`, `NMI_MASK`, `PREEMPT_ACTIVE` |
| Offset constants | `PREEMPT_OFFSET=1`, `SOFTIRQ_OFFSET=0x100`, `HARDIRQ_OFFSET=0x10000`, `NMI_OFFSET=0x100000` |
| `preempt_count()` | Read current task's raw preempt_count via `sched::current()` |
| `in_interrupt()` | `(HARDIRQ \| SOFTIRQ \| NMI) != 0` |
| `in_irq()` | `HARDIRQ_MASK != 0` |
| `in_softirq()` | `SOFTIRQ_MASK != 0` |
| `in_task()` | `!in_interrupt()` |
| `preemptible()` | `preempt_count == 0` |
| `preempt_count_add(val)` | Atomic fetch-add on current task |
| `preempt_count_sub(val)` | Atomic fetch-sub on current task |
| `irq_enter()` | `preempt_count += HARDIRQ_OFFSET` |
| `irq_exit()` | `preempt_count -= HARDIRQ_OFFSET` |

**Modified files:**

| File | Change |
|------|--------|
| `kernel/src/interrupt/mod.rs` | Added `pub mod preempt;` + re-exports |
| `kernel/src/process/task.rs` | `inc/dec_preempt_count()` use `PREEMPT_OFFSET`, added `add/sub_preempt_count()` public methods |
| `kernel/src/arch/riscv64/trap.rs` | Timer/soft/external handlers wrapped with `irq_enter()`/`irq_exit()` |
| `kernel/src/arch/riscv64/pt_regs.rs` | `in_interrupt()` delegates to `interrupt::preempt::in_interrupt()` |
| `kernel/src/arch/riscv64/mm/exception.rs` | Same delegation for local `in_interrupt()` |
| `kernel/src/arch/riscv64/trap.S` | Added `ti_preempt_count != 0` check before `asm_schedule` call in `.Lcheck_signals_and_resched` |

### Phase 6: Timer Interrupt Fix (Priority: High) — NEXT

> Goal: Enable scheduler tick, fix time-slice preemption

**Modified files:** `kernel/src/arch/riscv64/trap.rs`, `kernel/src/drivers/timer/riscv64.rs`

| Change | Description |
|--------|-------------|
| Uncomment `scheduler_tick()` | Call in `handle_timer_interrupt()` |
| Register as per-CPU IRQ | `request_percpu_irq(RV_IRQ_TIMER, timer_handler)` |
| Add `SCHED_SOFTIRQ` | `scheduler_tick()` triggers softirq for load balancing |
| Process time accounting | Update `utime`/`stime`, `account_process_tick()` |

### Phase 7: IPI Enhancement (Priority: Low) — TODO

> Goal: Support more IPI types, multiplex over soft interrupt

**Modified file:** `kernel/src/arch/riscv64/ipi.rs`

| Change | Description |
|--------|-------------|
| IPI type expansion | Reschedule, Call function, CPU stop, IRQ work |
| IPI multiplex | Single SBI IPI carries bitmap, send multiple types at once |
| Call function | `smp_call_function()` cross-CPU function call |
| IPI handler registration | Via `irq_domain`, no longer hardcoded |

### Phase 8: UART Interrupt-Driven I/O (Priority: Low) — TODO

**Modified files:** `kernel/src/console.rs`, `kernel/src/drivers/intc/plic.rs`

| Change | Description |
|--------|-------------|
| RX interrupt handling | IRQ 10 handler reads receive FIFO |
| Input buffer | Ring buffer to store received characters |
| Wake waiting process | `wait_queue` wakes processes blocked on `read()` |
| Remove polling | `getchar()` reads from buffer + blocking wait |

### Phase 9: NMI Support (Priority: Low) — TODO

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
Phase 1 (IRQ Framework) ✅ ──┬──→ Phase 3 (Softirq/Tasklet) ──→ Phase 4 (Threaded)
                               │
Phase 5 (preempt_count) ✅ ───┤
                               │
Phase 6 (Timer Fix) ──────────┤
                               │
Phase 2 (IRQ Stack) ──────────┼──→ Phase 7 (IPI Enhancement)
                               │
                               └──→ Phase 8 (UART Interrupt) ──→ Phase 9 (NMI)
```

### Recommended Implementation Order

| Stage | Content | Effort | Dependencies | Status |
|-------|---------|--------|--------------|--------|
| **Phase 1** | IRQ framework core | Large | None | ✅ DONE |
| **Phase 5** | preempt_count | Small | None, independent | ✅ DONE |
| **Phase 6** | Timer fix | Small | None, independent | Next |
| **Phase 3** | Softirq/Tasklet | Large | Depends on Phase 1 ✅ + Phase 5 | After 5+6 |
| **Phase 2** | Interrupt stack enhancement | Medium | None | After Phase 3 |
| **Phase 4** | Threaded interrupts | Large | Depends on Phase 3 | After Phase 3 |
| **Phase 7** | IPI enhancement | Medium | Depends on Phase 1 ✅ | After Phase 3 |
| **Phase 8** | UART interrupt-driven | Medium | Depends on Phase 1 ✅ | After Phase 3 |
| **Phase 9** | NMI support | Small | Depends on Phase 1 ✅ | Last |

**Recommended: Phase 5 + 6 next (quick independent fixes), then Phase 3 (softirq, the biggest value-add), then Phase 2 + 4, finally Phase 7-9 (polish).**

---

## 5. Kernel Big Lock Exit Path

The current `KERNEL_LOCK` is the root cause of all kernel code serialization. Interrupt subsystem refactoring creates conditions for big lock removal:

| Stage | Replacement Lock | Status |
|-------|-----------------|--------|
| Phase 1 complete | `irq_desc.lock` replaces global lock for IRQ operations | ✅ Done |
| Phase 3 complete | softirq needs no big lock (per-CPU data) | TODO |
| Phase 5 complete | preempt_count enables safe preemption | ✅ Done |
| Final | `rq->lock`, `irq_desc->lock`, `mm->mmap_lock` etc. fully replace big lock | TODO |

---

## 6. Affected Files

### New Files

| File | Phase | Status |
|------|-------|--------|
| `kernel/src/interrupt/mod.rs` | 1 | ✅ Created |
| `kernel/src/interrupt/irqdesc.rs` | 1 | ✅ Created |
| `kernel/src/interrupt/irqchip.rs` | 1 | ✅ Created |
| `kernel/src/interrupt/domain.rs` | 1 | ✅ Created |
| `kernel/src/interrupt/softirq.rs` | 3 | TODO |
| `kernel/src/interrupt/tasklet.rs` | 3 | TODO |
| `kernel/src/interrupt/ksoftirqd.rs` | 3 | TODO |
| `kernel/src/interrupt/thread.rs` | 4 | TODO |
| `kernel/src/interrupt/nmi.rs` | 9 | TODO |

### Modified Files

| File | Phase | Change Scope | Status |
|------|-------|-------------|--------|
| `kernel/src/main.rs` | 1 | Add `mod interrupt;` + boot init | ✅ Done |
| `kernel/src/drivers/intc/plic.rs` | 1 | IrqChip + IrqDomainOps implementation | ✅ Done |
| `kernel/src/arch/riscv64/trap.rs` | 1,5,6 | IRQ dispatch + irq_enter/irq_exit | ✅ Phase 1+5 done |
| `kernel/src/drivers/virtio/mod.rs` | 1 | Handler signature + request_irq | ✅ Done |
| `kernel/src/drivers/virtio/virtio_pci.rs` | 1 | request_irq registration | ✅ Done |
| `kernel/src/drivers/net/virtio_net.rs` | 1 | Handler signature + request_irq | ✅ Done |
| `kernel/src/arch/riscv64/ipi.rs` | 1,7 | request_irq for IPI handlers | ✅ Phase 1 done |
| `kernel/src/fs/procfs/interrupts.rs` | 1 | Read from irq_desc counters | ✅ Done |
| `kernel/src/arch/riscv64/trap.S` | 2,5 | Interrupt stack + preempt_count | ✅ Phase 5 done |
| `kernel/src/arch/riscv64/pt_regs.rs` | 5 | `in_interrupt()` delegates to preempt module | ✅ Done |
| `kernel/src/arch/riscv64/smp.rs` | 2 | Overflow detection | TODO |
| `kernel/src/drivers/timer/riscv64.rs` | 6 | Timer via IRQ framework | TODO |
| `kernel/src/console.rs` | 8 | UART interrupt-driven RX/TX | TODO |
| `kernel/src/process/task.rs` | 5 | `ti_preempt_count` actual usage | ✅ Done |
| `kernel/src/sched/sched.rs` | 5,6 | preempt_count + scheduler_tick | ✅ Phase 5 done |
| `kernel/src/net/tcp_timer.rs` | 3 | Migrated to TIMER_SOFTIRQ | TODO |
