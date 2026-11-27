//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 trap handling — GDT/TSS/IDT, syscall MSRs, PIC 8259, and the
//! Rust trap dispatch (port of the riscv64 twin's trap.rs).
//!
//! ## Frame contract
//!
//! trap.S builds a uniform 168-byte pt_regs for every entry (see the
//! layout comment there); user-origin frames land exactly at
//! `stack_top - sizeof(PtRegs)`, which is what `Task::pt_regs()` and
//! fork rely on.  The offsets are pinned by the static asserts below.
//!
//! ## GDT / selectors (same indices as the boot.S bootstrap GDT)
//!
//! | idx | contents        | selector |
//! |-----|-----------------|----------|
//! | 0   | null            | —        |
//! | 1   | 64-bit code     | 0x08     |
//! | 2   | data            | 0x10     |
//! | 5   | user data       | 0x2b     |
//! | 6   | 64-bit user code| 0x33     |
//! | 7/8 | TSS (16-byte)   | 0x38     |
//!
//! STAR = (0x23<<48) | (0x08<<32): SYSCALL enters on CS=0x08/SS=0x10;
//! SYSRET64 returns to CS = STAR[63:48]+16 = 0x33, SS = +8 = 0x2b.
//! FMASK = TF|IF|DF — syscalls enter with IF clear; `handle_syscall`
//! re-enables interrupts like the riscv64 twin.  Returns use SYSRET on
//! the untouched fast path (trap.S trap_exit_syscall; rcx/r11 mirror
//! checks) and IRETQ everywhere else — the Linux discipline.


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use super::unsafe_edges::SavedFrame;
use super::cpu::{inb, outb, rdmsr, wrmsr};
use crate::arch::pt_regs::{Cause, PtRegs, PT_REGS_SIZE};
use crate::sync::spinlock::Spinlock;

// Include trap.S assembly code (AT&T syntax — global_asm defaults to
// Intel on x86 targets).


// ============================================================================
// ret_from_fork / ret_from_fork_kernel — fork/kthread first-instruction
// entries as Rust naked fns (moved out of trap.S): these two are the only
// trap.S symbols whose ADDRESS is taken from Rust (fork.rs, init.rs,
// kthread.rs thread_set_entry), and under any LTO setting the linker
// instantiated the include_str!'d global_asm! copy of trap.S more than
// once, defining the address-taken symbol twice and failing the release
// profile build (pa39 attempts 1-2, pa40 thin-LTO rerun — "symbol
// 'ret_from_fork' is already defined"). Naked fns are ordinary Rust
// functions: LTO-safe by construction. Bodies are the trap.S originals
// byte-for-byte (AT&T syntax); every callee is a plain symbol
// (#[no_mangle] get_prev_task / schedule_tail / ret_from_fork_kernel_helper,
// and trap_exit exported .global/.hidden from trap.S).
// ============================================================================



// Table-load helpers (Intel-syntax inline quirks make the reload dance
// easier as one asm unit: lgdt + CS reload + segment reloads + ltr).


// ============================================================================
// Static asserts: pin the pt_regs layout trap.S was written against
// ============================================================================

pub(crate) const _: () = assert!(PT_REGS_SIZE == 0xa8);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, r15) == 0x00);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, r14) == 0x08);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, r13) == 0x10);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, r12) == 0x18);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, rbp) == 0x20);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, rbx) == 0x28);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, r11) == 0x30);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, r10) == 0x38);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, r9) == 0x40);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, r8) == 0x48);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, rax) == 0x50);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, rcx) == 0x58);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, rdx) == 0x60);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, rsi) == 0x68);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, rdi) == 0x70);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, orig_rax) == 0x78);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, rip) == 0x80);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, cs) == 0x88);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, rflags) == 0x90);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, rsp) == 0x98);
pub(crate) const _: () = assert!(core::mem::offset_of!(PtRegs, ss) == 0xa0);

// Selector constants (GDT indices pinned above).
pub const KERNEL_CS: u64 = 0x08;
pub const KERNEL_DS: u64 = 0x10;
pub const USER_CS: u64 = 0x33;
pub const USER_DS: u64 = 0x2b;
pub const TSS_SELECTOR: u16 = 0x38;

// TSS field offsets (byte offsets inside the 0x68-byte TSS).
pub const TSS_OFF_RSP0: usize = 0x04;
pub const TSS_OFF_SP2: usize = 0x14;
pub const TSS_OFF_IST1: usize = 0x24;
pub const TSS_SIZE: usize = 0x68;

pub(crate) const _: () = assert!(TSS_OFF_RSP0 == 4 && TSS_OFF_SP2 == 0x14 && TSS_OFF_IST1 == 0x24);

/// The syscall vector used between trap.S and trap_handler (not an IDT
/// entry — syscalls arrive through LSTAR).
pub const SYSCALL_VECTOR: u64 = 0x80;

/// STAR value: kernel CS 0x08 on SYSCALL; user base 0x23 (the user-code
/// GDT index 6 << 3 | RPL3) so SYSRET64 computes CS = 0x23+16 = 0x33
/// (user code64) and SS = 0x23+8 = 0x2b — both with RPL 3. The base
/// used to be 0x20, which SYSRET would have loaded as CS=0x30/SS=0x28
/// (RPL 0); nothing exercised it while returns were iretq-only
/// (gap-net added the SYSRET tail in trap.S).
pub(crate) const MSR_STAR_VALUE: u64 = (0x23u64 << 48) | (0x08u64 << 32);
/// FMASK: TF(8) | IF(9) | DF(10) cleared on syscall entry.
pub(crate) const MSR_FMASK_VALUE: u64 = (1 << 8) | (1 << 9) | (1 << 10);

pub(crate) const MSR_EFER: u32 = 0xC000_0080;
pub(crate) const MSR_STAR: u32 = 0xC000_0081;
pub(crate) const MSR_LSTAR: u32 = 0xC000_0082;
pub(crate) const MSR_FMASK: u32 = 0xC000_0084;
pub(crate) const EFER_SCE: u64 = 1;
/// EFER.NXE — without it every PTE carrying NX (all non-executable user
/// mappings: stacks, data, bss) raises a reserved-bit #PF (ec bit 3) on
/// ANY access, which the dispatcher then misreads as a read fault and
/// "handles" into an infinite loop.
pub(crate) const EFER_NXE: u64 = 1 << 11;

// ============================================================================
// GDT / TSS / IDT tables
// ============================================================================

/// GDTR/LIDTR pseudo-descriptor (packed: 10 bytes).
#[repr(C, packed)]
pub(crate) struct PseudoDescriptor {
    pub(crate) limit: u16,
    pub(crate) base: u64,
}

// asm table-load helpers defined in the global_asm! block below.
extern "C" {
    fn x86_load_gdt(pd: *const PseudoDescriptor);
    fn x86_load_idt(pd: *const PseudoDescriptor);
}

/// 16-byte IDT gate descriptor (64-bit mode).
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct IdtEntry {
    pub(crate) offset_low: u16,
    pub(crate) selector: u16,
    pub(crate) ist: u8,      // bits 0..2 = IST index, rest 0
    pub(crate) type_attr: u8, // P<<7 | DPL<<5 | 0<<4 | gate type (0xE = interrupt)
    pub(crate) offset_mid: u16,
    pub(crate) offset_high: u32,
    pub(crate) reserved: u32,
}

impl IdtEntry {
    pub(crate) const fn empty() -> Self {
        IdtEntry {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    pub(crate) fn set_handler(&mut self, addr: u64, ist: u8, dpl: u8) {
        self.offset_low = (addr & 0xFFFF) as u16;
        self.selector = KERNEL_CS as u16;
        self.ist = ist & 0x7;
        self.type_attr = 0x80 | ((dpl & 0x3) << 5) | 0x0E; // present, interrupt gate
        self.offset_mid = ((addr >> 16) & 0xFFFF) as u16;
        self.offset_high = ((addr >> 32) & 0xFFFF_FFFF) as u32;
        self.reserved = 0;
    }
}

/// 64-bit TSS (104 bytes).  Accessed through byte offsets so the packed
/// layout never produces unaligned references.
#[repr(C, align(8))]
pub struct Tss {
    pub(crate) bytes: [u8; TSS_SIZE],
}

impl Tss {
    pub(crate) const fn zeroed() -> Self {
        Tss { bytes: [0; TSS_SIZE] }
    }


}

/// Per-CPU TSS tables (the TSS referenced by each CPU's own GDT at
/// selector 0x38; IST1 = per-CPU double-fault stack, rsp0 = the current
/// task's kernel stack top).
pub(crate) static mut TSS_PER_CPU: [Tss; crate::config::MAX_CPUS] =
    [const { Tss::zeroed() }; crate::config::MAX_CPUS];

/// IST1 stack for the double-fault handler (16K per CPU, .bss.stack so
/// it sits with the other privileged stacks).
#[repr(align(16))]
pub(crate) struct DoubleFaultStack(pub(crate) [u8; 16384]);


/// GDT: null, code64, data, (2 unused), user data, user code64, TSS(16B).
#[repr(C, align(8))]
pub(crate) struct Gdt(pub(crate) [u64; 9]);

/// Segment descriptor helper: base=0, limit=0xFFFF, flags in the top byte.
pub(crate) const fn seg_desc(access: u8, flags: u8) -> u64 {
    ((flags as u64) << 52) | ((access as u64) << 40) | 0xFFFF
}

/// Per-CPU GDTs (each points at that CPU's TSS through entries 7/8).
pub(crate) static mut GDT_PER_CPU: [Gdt; crate::config::MAX_CPUS] = [const {
    Gdt([
        0,
        seg_desc(0x9A, 0xA), // 0x08: code64 (L=1, D=0) — 0x00AF9A000000FFFF
        seg_desc(0x92, 0xC), // 0x10: data — 0x00CF92000000FFFF
        0,
        0,
        seg_desc(0xF2, 0xC), // 0x2b: user data (DPL 3)
        seg_desc(0xFA, 0xA), // 0x33: user code64 (DPL 3)
        0,                   // 0x38: TSS low word (filled at init)
        0,                   //      TSS high word (base[63:32])
    ])
}; crate::config::MAX_CPUS];

/// IDT — 256 gates; built at init from the trap.S stub symbols.
pub(crate) static mut IDT_X86: [IdtEntry; 256] = [const { IdtEntry::empty() }; 256];
/// Update TSS.rsp0 (the kernel stack top user entries switch to) and
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-69): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn set_tss_rsp0(top: u64) {
    crate::arch::arch_floor::x86_64::trap::set_tss_rsp0(top)
}

// ============================================================================
// init
// ============================================================================

/// Build and load this CPU's GDT/TSS (same selector layout on every
/// CPU; the TSS descriptor points at THIS CPU's TSS), then re-assert
/// the GS base (lgdt loads a null GS selector; the architectural GS
/// base survives that, but re-writing it keeps the invariant explicit).
// IRREDUCIBLE F-ROW (F-BOOT-X1): per-CPU GDT/TSS bring-up (static-mut
// per-CPU tables + lgdt/ltr) — reference kernel arch/x86/kernel/cpu/
// common.c cpu_init_exception_handling; boot-section ownership.
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-68): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn init_cpu_state(cpu: usize) {
    crate::arch::arch_floor::x86_64::trap::init_cpu_state(cpu)
}
/// Initialize trap handling on the boot CPU: FPU, per-CPU GDT/TSS, the
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-67): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn init() {
    crate::arch::arch_floor::x86_64::trap::init()
}
/// Per-CPU trap bring-up on an AP: FPU, its own GDT/TSS (LTR), the
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-66): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn init_secondary(cpu: usize) {
    crate::arch::arch_floor::x86_64::trap::init_secondary(cpu)
}
/// Initialize syscall MSRs (STAR/LSTAR/FMASK) and EFER.SCE.
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-65): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn init_syscall() {
    crate::arch::arch_floor::x86_64::trap::init_syscall()
}

// ============================================================================
// PIC 8259
// ============================================================================

pub(crate) const PIC1_CMD: u16 = 0x20;
pub(crate) const PIC1_DATA: u16 = 0x21;
pub(crate) const PIC2_CMD: u16 = 0xA0;
pub(crate) const PIC2_DATA: u16 = 0xA1;

/// PIC IRQ base vectors (master 0x20-0x27, slave 0x28-0x2f).
pub const PIC_IRQ_BASE: u8 = 0x20;

/// Legacy PC/AT IRQ line numbers.
pub const IRQ_TIMER: u8 = 0;
pub const IRQ_CASCADE: u8 = 2;

/// Standard Linux-style init_8259A: ICW1-4 remap to vectors 32..47,
/// cascade wiring, then mask everything except the cascade line.
// IRREDUCIBLE F-ROW (F-BOOT-X2): the 8259 PIC ICW program sequence —
// reference kernel arch/x86/kernel/i8259.c init_8259A; port-IO order
// IS the protocol.
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-64): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn init_8259() {
    crate::arch::arch_floor::x86_64::trap::init_8259()
}
/// Non-specific EOI — conservative like early Linux: always EOI the
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-63): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn pic_send_eoi(line: u8) {
    crate::arch::arch_floor::x86_64::trap::pic_send_eoi(line)
}
/// Claim the highest-priority in-service IRQ (PIC analogue of the PLIC
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-62): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn pic_claim() -> Option<u8> {
    crate::arch::arch_floor::x86_64::trap::pic_claim()
}

/// Serializes PIC mask-register RMWs. Leaf lock (never held across any
/// other acquire). Needed since RT-plan R5: per-line IRQ threads unmask
/// from task context (any CPU) while hardirq top halves mask on the BSP
/// — the old "IF-off or single-owner" contract no longer holds.
pub(crate) static PIC_MASK_LOCK: Spinlock<()> = Spinlock::new(());
/// Unmask one PIC line (0..15).
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-61): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn unmask_irq_line(line: u8) {
    crate::arch::arch_floor::x86_64::trap::unmask_irq_line(line)
}
/// Mask one PIC line (0..15).
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-60): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn mask_irq_line(line: u8) {
    crate::arch::arch_floor::x86_64::trap::mask_irq_line(line)
}

// ============================================================================
// IRQ line dispatch table (trap.rs-owned; the generic irq-domain layer
// is PLIC-shaped and arrives with the LAPIC/IOAPIC phase)
// ============================================================================

pub(crate) type IrqLineHandler = fn();

pub(crate) static IRQ_LINE_HANDLERS: [core::sync::atomic::AtomicU64; 16] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 16];

/// The line owners' names (the irq:irq_handler_entry name field; the
/// x86 PIC path predates the irq-domain action layer, so the name rides
/// a parallel registry set at request time).
pub(crate) static IRQ_LINE_NAMES: [Spinlock<&'static str>; 16] = [const { Spinlock::new("") }; 16];

/// Register a handler for a PIC line (0..15) and unmask it.  Returns
/// false when the line is taken or out of range.
pub fn request_irq_line(line: u8, handler: fn()) -> bool {
    request_irq_line_named(line, "", handler)
}

/// `request_irq_line` with the owner's name (what the
/// irq:irq_handler_entry tracepoint reports; the empty name renders as
/// the bare line number prefix the irq event's comm slot carries).
pub fn request_irq_line_named(line: u8, name: &'static str, handler: fn()) -> bool {
    use core::sync::atomic::Ordering;
    if line >= 16 {
        return false;
    }
    let h = handler as u64;
    if IRQ_LINE_HANDLERS[line as usize]
        .compare_exchange(0, h, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    *IRQ_LINE_NAMES[line as usize].lock_irqsave() = name;
    // RT-plan R5: forced threading spawns the per-line service thread at
    // registration (the ksoftirqd lifecycle shape; unbound kthread, no
    // R86 bind race). The line still unmasks — threading changes the
    // dispatch context, not the registration contract.
    if crate::interrupt::irqdesc::force_threaded() {
        spawn_intx_irq_thread(line, name);
    }
    unmask_irq_line(line);
    true
}

/// The registered name of a line ("" when free or registered unnamed).
pub fn irq_line_name(line: u8) -> &'static str {
    if line >= 16 {
        return "";
    }
    *IRQ_LINE_NAMES[line as usize].lock()
}

/// Fallback names for lines registered without one ("line<N>", what an
/// unnamed action reports — the reference always carries an action name).
pub(crate) const LINE_NAME_FALLBACK: [&str; 16] = [
    "line0", "line1", "line2", "line3", "line4", "line5", "line6", "line7", "line8",
    "line9", "line10", "line11", "line12", "line13", "line14", "line15",
];

pub fn free_irq_line(line: u8) {
    use core::sync::atomic::Ordering;
    if line < 16 {
        mask_irq_line(line);
        IRQ_LINE_HANDLERS[line as usize].store(0, Ordering::Release);
        *IRQ_LINE_NAMES[line as usize].lock_irqsave() = "";
    }
}

// ============================================================================
// Threaded IRQ services (RT-plan R5, forced threading on x86_64)
//
// The reference's threadirqs shape for the x86_64 paths that predate the
// irq-domain action layer: the virtio-blk MSI-X completion vector and the
// PIC INTx lines. The hardirq top half only suppresses re-fire and wakes
// a per-IRQ SCHED_FIFO 50 kthread; the action runs preemptibly in the
// thread. The timer/clock vectors and IPIs never use this — they stay
// raw hardirq (wake-type timer expiry is the RT latency path).
// ============================================================================

use crate::interrupt::irqthread::ThreadedIrq;
/// Invoke a line's action inline with the irq_handler_entry/exit
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-59): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn run_line_action(line: u8, h: u64) {
    crate::arch::arch_floor::x86_64::trap::run_line_action(line, h)
}

/// The per-PIC-line service threads ("irq/<line>-<name>").
pub(crate) static INTX_IRQ_THREADS: [ThreadedIrq; 16] = [const { ThreadedIrq::new() }; 16];

/// The virtio-blk MSI-X completion thread ("irq/129-blk", vector 0x81).
pub(crate) static BLK_MSIX_IRQ_THREAD: ThreadedIrq = ThreadedIrq::new();

/// Per-line INTx service thread body: run the line's handler (the ISR
/// read inside dispatches/ACKs the asserting virtio functions; the UART
/// handler drains the RX FIFO), then unmask. Unmask order matters: the
/// handler's device-side ACK drops the level assertion first, so the
/// unmask cannot release a storm (the reference's threaded level-IRQ
/// completion shape: thread runs action, then chip re-arms).
pub(crate) extern "C" fn intx_irq_thread_fn(arg: *mut core::ffi::c_void) -> i32 {
    let line = (arg as usize) as u8;
    INTX_IRQ_THREADS[line as usize].thread_loop(|| {
        let h = IRQ_LINE_HANDLERS[line as usize].load(core::sync::atomic::Ordering::Acquire);
        if h != 0 {
            run_line_action(line, h);
        }
        unmask_irq_line(line);
    })
}

/// MSI-X completion thread body: the per-disk walker that used to run
/// inline in the hardirq gate. No EOI/unmask here — MSI is edge
/// delivered, the top half EOIs first (a completion arriving while the
/// thread runs delivers a fresh vector and re-pends, coalesced by the
/// pending flag; the walker drains the whole used ring anyway).
pub(crate) extern "C" fn blk_msix_irq_thread_fn(_arg: *mut core::ffi::c_void) -> i32 {
    BLK_MSIX_IRQ_THREAD.thread_loop(crate::drivers::virtio::msix_blk_isr)
}
/// Spawn (idempotent) the per-line thread at registration time. The
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-58): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn spawn_intx_irq_thread(line: u8, name: &'static str) -> bool {
    crate::arch::arch_floor::x86_64::trap::spawn_intx_irq_thread(line, name)
}

/// The MSI-X vector was programmed (probe-time marker for the deferred
/// boot pass — the spawn itself may have no-op'd pre-scheduler).
pub(crate) static BLK_MSIX_THREAD_WANTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Spawn (idempotent) the virtio-blk MSI-X completion thread. Called by
/// the virtio PCI probe once vector 0x81 is programmed into the MSI-X
/// table (the registration point for this pseudo-IRQ). The blk queue is
/// an IRQF_NO_THREAD-class opt-out: only `irqthread=all` threads it
/// (measured -28% 4k randwrite at the default shape; see run/RT-R5.md).
pub fn spawn_blk_msix_irq_thread() {
    if !crate::interrupt::irqdesc::force_thread_even_optouts() {
        return;
    }
    BLK_MSIX_THREAD_WANTED.store(true, core::sync::atomic::Ordering::Release);
    BLK_MSIX_IRQ_THREAD.spawn(
        "irq/129-blk",
        blk_msix_irq_thread_fn,
        core::ptr::null_mut(),
    );
}

/// Deferred per-IRQ thread pass (post-sched::init; the ksoftirqd
/// precedent): registrations that ran before the scheduler existed left
/// their lines on the inline fallback — spawn their threads now.
/// Returns how many threads spawned (boot-log visibility).
pub fn irq_threads_boot_init() -> usize {
    let mut n = 0usize;
    for line in 0..16u8 {
        if IRQ_LINE_HANDLERS[line as usize].load(core::sync::atomic::Ordering::Acquire) != 0 {
            if spawn_intx_irq_thread(line, irq_line_name(line)) {
                n += 1;
            }
        }
    }
    if BLK_MSIX_THREAD_WANTED.load(core::sync::atomic::Ordering::Acquire) {
        if BLK_MSIX_IRQ_THREAD.spawn(
            "irq/129-blk",
            blk_msix_irq_thread_fn,
            core::ptr::null_mut(),
        ) {
            n += 1;
        }
    }
    n
}

// ============================================================================
// Interrupt control (doc-comment contracts from the skeleton)
// ============================================================================
/// Enable the timer interrupt: arm the per-CPU LAPIC timer when the
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-57): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn enable_timer_interrupt() {
    crate::arch::arch_floor::x86_64::trap::enable_timer_interrupt()
}

/// Disable the timer interrupt (mask the LAPIC timer, or PIC IRQ0 on
/// the no-APIC fallback path).
pub fn disable_timer_interrupt() {
    if crate::drivers::intc::apic::ready() {
        crate::drivers::intc::apic::timer_mask();
    } else {
        mask_irq_line(IRQ_TIMER);
    }
}

/// Enable external interrupts.  x86 has no per-source CPU gate (sie):
/// IF is the gate and PIC lines are unmasked individually via
/// `request_irq_line`/`unmask_irq_line`; the cascade line is already
/// open after init_8259.
pub fn enable_external_interrupt() {}

// ============================================================================
// Per-CPU pt_regs (fork support) — same nesting protocol as the twin
// ============================================================================

/// Current CPU's pt_regs pointer (used by fork) — per-CPU slots
pub(crate) static CURRENT_PT_REGS: [core::sync::atomic::AtomicU64; crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; crate::config::MAX_CPUS];

pub fn current_pt_regs() -> *const PtRegs {
    let cpu = crate::arch::cpu_id() as usize;
    CURRENT_PT_REGS[cpu].load(core::sync::atomic::Ordering::Relaxed) as *const PtRegs
}
/// Wakewatch: RIP of another CPU's most recent trap frame, for the
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-56): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn pt_regs_rip_for_cpu(cpu: usize) -> u64 {
    crate::arch::arch_floor::x86_64::trap::pt_regs_rip_for_cpu(cpu)
}
/// pt_regs of the current task's user frame (stack_top - sizeof(PtRegs))
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-55): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn current_task_pt_regs() -> Option<&'static mut PtRegs> {
    crate::arch::arch_floor::x86_64::trap::current_task_pt_regs()
}

/// Is `frame` inside `task`'s kernel stack [bottom, top)?  False for null
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-53): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn frame_in_task_stack(task: *mut crate::process::task::Task, frame: usize) -> bool {
    crate::arch::arch_floor::x86_64::trap::frame_in_task_stack(task, frame)
}

pub(crate) fn scribble_put_hex(mut v: u64) {
    for _ in 0..16 {
        let n = (v >> 60) as u8;
        crate::console::putchar_no_lock(if n < 10 { b'0' + n } else { b'a' + n - 10 });
        v <<= 4;
    }
}

pub(crate) fn scribble_put_str(s: &[u8]) {
    for &b in s {
        crate::console::putchar_no_lock(b);
    }
}
/// Raw (lock-free) report of a user-origin trap frame landing outside the
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-52): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn scribble_foreign_entry( cpu: usize, vector: u64, regs: *mut PtRegs, tp: *mut crate::process::task::Task, frame: usize, ) {
    crate::arch::arch_floor::x86_64::trap::scribble_foreign_entry(cpu, vector, regs, tp, frame)
}

// ============================================================================
// Trap dispatch
// ============================================================================

/// x86 vector numbers used in the match below.
pub(crate) mod vec {
    pub const DE: u64 = 0; // divide error
    pub const DB: u64 = 1; // debug
    pub const NMI: u64 = 2;
    pub const BP: u64 = 3; // breakpoint (int3)
    pub const OF: u64 = 4; // overflow (into)
    pub const BR: u64 = 5; // bound range
    pub const UD: u64 = 6; // invalid opcode
    pub const NM: u64 = 7; // device not available
    pub const DF: u64 = 8; // double fault (never routed here - IST stub)
    pub const TS: u64 = 10;
    pub const NP: u64 = 11;
    pub const SS: u64 = 12;
    pub const GP: u64 = 13;
    pub const PF: u64 = 14;
    pub const MF: u64 = 16; // x87 math fault
    pub const AC: u64 = 17; // alignment check
    pub const XM: u64 = 19; // SIMD (SSE) math fault
    pub const CP: u64 = 21; // control protection
    pub const IRQ0: u64 = 32; // PIC timer vector (virtual-wire fallback)
    pub const IRQ_LAST: u64 = 47; // PIC slave line 15 vector
}

/// LAPIC timer vector (per-CPU tick; same handler body as the PIC path).
pub(crate) const LOCAL_TIMER: u64 = crate::drivers::intc::apic::LOCAL_TIMER_VECTOR as u64;


/// Rate gate for the IRQ-OFF-WINDOW probe: at most one direct-console
/// report per 2 s of jiffies (episodes can cluster; the log must not).
/// sc167: the 20-jiffy (200 ms) gate let sc166 print 2300 windows, and
/// every report is itself a ~130-char direct-console line whose THRE
/// waits run with IF=0 — the probe was feeding the very storm it
/// measured. 200 jiffies restores the documented 2 s cadence.
pub(crate) fn ww_irqwin_budget_ok() -> bool {
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let now = crate::drivers::timer::get_jiffies();
    let last = LAST.load(core::sync::atomic::Ordering::Acquire);
    if now.wrapping_sub(last) < 200 {
        return false;
    }
    LAST.compare_exchange(last, now, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Acquire)
        .is_ok()
}

/// Kernel-mode preemption at IRQ exit (CONFIG_PREEMPT equivalent).
///
/// First landed as 2cf4bdc9 and reverted in 0ced11da after it poisoned a
/// task's preempt_count to -1 (user page faults then read
/// in_interrupt()==true → KERNPANIC, cpu0 parked, softirq starved for
/// minutes). Root cause of that failure: the hook called schedule() with
/// NO re-entry guard, so a timer tick landing inside __schedule's own
/// tail — after the incoming task's saved IF was restored — re-entered
/// schedule() on the same, half-unwound kernel stack, and the nested
/// entry ran the prev-resolution/pick/heal machinery on frames that were
/// still unwinding (double-run shapes; unpaired guard drops).
///
/// Re-landing round 1 (sc65/sc66) used a PREEMPT_ACTIVE bit inside
/// preempt_count as the guard, Linux-style. That was falsified for this
/// tree: residual cross-task count imbalances exist in the pick/heal
/// machinery (sc66 strand incumbents showed init carrying 0x04000001 and
/// 0x08000001 — leaked ACT bits plus a leaked PREEMPT_OFFSET), and every
/// leaked count bit now has teeth: preemptible() stays false forever, so
/// the task cannot be preempted, AND __schedule's switch-out discipline
/// refuses to sleep it — the observed 160-291 s one-activation
/// monopolies. Round 2 therefore keeps the count EXACTLY as it was
/// before this work:
///
/// - schedule() sets the TIF_IN_SCHED ti_flags bit around __schedule.
///   This hook refuses while the bit is set — the only re-entry point is
///   this hook (the trap-exit asm loop, cond_resched and the SleepMutex
///   slowpath all call schedule() sequentially, never from inside
///   another one). A leaked flag, should one ever happen, only disables
///   preemption for that task; count semantics and the sleep discipline
///   stay exact.
/// - sub_preempt_count clamps and reports underflow, so a residual
///   unpaired decrement is one attributable log line, never a silent -1
///   faking in_interrupt() for a user task.
///
/// Call contract (unchanged from 2cf4bdc9's analysis, still true):
/// - Called from the timer/IPI vector paths AFTER irq_exit(), with IF=0
///   (interrupt gate) and the hardirq count already subtracted.
/// - User-mode frames are skipped — trap_exit's asm loop owns those.
/// - preempt_count()==0 means the interrupted kernel context holds no
///   spinlock (all guards add PREEMPT_OFFSET), no bh section, no RCU
///   read side (rcu_read_lock = preempt_disable).
/// - The switch is the same one sleeping syscalls take; mid-syscall
///   migration already happens via blocking paths, and the per-caller
///   IRQ-flag discipline (R54) returns each side to its own IF state.
/// - After schedule() returns, the frame unwinds normally through
///   trap_handler's tail and trap_exit's kernel restore.
pub(crate) fn irq_exit_maybe_preempt_kernel(regs: &PtRegs) {
    use crate::process::task::task_flags::TaskFlags;
    // LANDING STATE after the sc69-sc73 A/B ladder:
    // - The preempt_count leak is PRE-EXISTING (sc69: hook compiled out,
    //   leak present; disassembly-verified) — a plain SpinlockGuard whose
    //   add and drop execute in two different task contexts while the
    //   victim runs. sc73's lock fingerprints name the hot instances:
    //   fs::inode::ICACHE, fs::locks::RECORD_LOCK_TABLE (statics) plus
    //   heap Spinlocks — the VFS-path double-run engine. Producer hunt
    //   continues (needs the pick/heal or scribble fix; not this hook).
    // - With the hook ON the leak is amplified: leaked tasks become
    //   kernel-mode monopolists (sc70: 309 s; sc73 with both heals still
    //   86 s / 29 heals). With the hook OFF the same leaks self-bound at
    //   user-mode boundaries (~2.6 s, sc69) and the heals bound the rest.
    // Therefore the hook stays OFF by default; PREEMPT-HEAL/PARKED-HEAL
    // stay as the safety net either way.
    //
    // kpreempt (cmdline): runtime arm of this hook. Same contract on
    // every arch: kpreempt=1 arms, kpreempt=0 disarms, ABSENT takes the
    // arch default. x86_64 default: ON (rt-r1 lane, 2026-11-18, RT-plan
    // stage R1 — docs/development/rt-kernel-plan.md). History: ON at
    // the perf-sched lane (2026-10-31), back to OFF at the wedge-fix
    // lane (2026-11-01, 28b1afca) after release-build OH SA storms
    // wedged with the hook armed (66 both-CPU `DEADLOCK: spinlock
    // stuck` prints on binder::WORLD; `kpreempt=0` on the same tree:
    // zero prints, clean-anchor-level SA progress — SAREG txn 7266 vs
    // clean anchor 33d007fe's 7213 at 120 s). The riscv64 twin still
    // defaults OFF (frame discipline unstabilized there — R46/R49).
    //
    // WHY the default can go back ON (R1 re-land, both documented
    // conditions now met on this tree):
    // (1) The wedge family's fuel was preempt_count leaks amplified by
    // the hook (sc70: leaked counts + hook ON = kernel-mode
    // monopolists). The heal stack that landed since — WAKE-HEAL
    // zeroing parked phantom counts at wake, the rotation-proof 2 s
    // tick heal, the 119th booked-task irq_enter/irq_exit release
    // (bracket-spanning switches release on the task that BOOKED the
    // base), the glib-mutex sub-before-unlock GRQ guard order, and the
    // phantom-bits softirq deferral out of irq_exit windows — bounds
    // every producer class the sc70 census could name; the standing
    // Tier-1 preempt-count census reads heal-miss 0
    // (perf-gate-baseline pa115debt1). The R1 merge gate re-verifies
    // with the hook armed: census bands hold + a release OH SA-storm
    // 600 s boot with zero DEADLOCK prints.
    // (2) The p2f condition ("the hook alone restores the 9.5 µs
    // wake->run") was unsatisfiable on the 11-01 tip because the D-14
    // measured-wait gate never raised need_resched for batch peers.
    // R2's hardirq hres expiry (RT-2X) has since replaced the wake
    // path's delivery context; the R1 acceptance is the paired-test
    // cyclictest SCHED_OTHER avg (107 -> the 63-70 µs class against
    // the banked reference column), measured on the same rig.
    //
    // The sc70-era amplification verdict (hook ON + preempt_count leaks
    // = 309s kernel monopolies) predates the WAKE-HEAL (parked phantom
    // counts zeroed at wake before the monopoly can form) and the
    // rotation-proof 2s tick heal, so the amplifier's fuel is bounded.
    //
    // Default-ON evidence (perf-sched lane, pbench p2f futex suite,
    // TCG q35 smp2/2G and KVM, kernel = 159111af lineage + mprotect fix):
    // with a pread(4MB)-cached hog pinned to the woken waiter's CPU (one
    // syscall = one contiguous kernel stretch; hogcal TCG 5.5ms / KVM
    // 160us), futex wake->run was syscall-length-quantized:
    //   TCG: med 5608us (97% of wakes 1-10ms) vs reference 86us = 65x
    //   KVM: med 163us vs reference 8.2us = 20x
    // kpreempt=1 collapses it to reference parity (TCG 23us, KVM 9.5us)
    // and the full P1-P7 suite shows no row moved beyond noise
    // (KVM kp=0 vs kp=1 back-to-back: all rows within ±5%, p2_ctxsw
    // 0.18->0.175). Boot beacon delta on the same test suite: PREEMPT-HEAL
    // events went DOWN with the hook on (49 -> 2), zero WAKE-STRANDED.
    // The verdict-cache discipline mirrors syscost::on(): never cache a
    // verdict before the cmdline exists (the first vector can fire
    // pre-cmdline::init).
    static KP_ARMED: core::sync::atomic::AtomicU8 =
        core::sync::atomic::AtomicU8::new(0);
    match KP_ARMED.load(core::sync::atomic::Ordering::Relaxed) {
        0 => {
            match crate::cmdline::get_cmdline() {
                None => return, // cmdline not parsed yet: stay disarmed
                Some(_) => {
                    // R1 default flip: ABSENT now arms. kpreempt=0 is
                    // the explicit disarm (the old OFF-default escape
                    // hatch, kept for bisects and storm A/Bs).
                    let v = if crate::cmdline::get_param("kpreempt")
                        .as_deref()
                        == Some("0")
                    {
                        1u8
                    } else {
                        2u8
                    };
                    KP_ARMED.store(v, core::sync::atomic::Ordering::Relaxed);
                    if crate::cmdline::get_param("kpreempt").as_deref()
                        == Some("1")
                    {
                        // HOTPATH-PRINT: budget-gated (the budget/guard above).
        crate::pr_info!(
                            "sched: kernel-preempt armed by cmdline (cpu={})",
                            crate::arch::cpu_id()
                        );
                    }
                }
            }
        }
        1 => return, // disarmed (explicit kpreempt=0; the R1 default arms)
        _ => {}
    }
    // Early-boot / WFI secondaries have no current task: __schedule
    // handles a null prev, but there is nothing to preempt either way.
    let cur = match crate::sched::current() {
        Some(t) => t,
        None => return,
    };
    if regs.user_mode() {
        return; // user path: trap_exit asm owns preemption
    }
    if !crate::sched::need_resched() {
        return;
    }
    if !crate::interrupt::preempt::preemptible() {
        return; // holds a lock / bh / RCU / nested IRQ
    }
    // Recursion guard: never re-enter a schedule() already in progress on
    // this task (a tick can land in __schedule's tail after the incoming
    // task's saved IF was restored). TIF_IN_SCHED is a task flag, not a
    // count bit — a stale set only disables preemption for this task.
    if cur.test_ti_flag(TaskFlags::TIF_IN_SCHED.bits()) {
        return;
    }
    static FIRED: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);
    if !FIRED.swap(true, core::sync::atomic::Ordering::AcqRel) {
        // HOTPATH-PRINT: preempt-leak report — one-shot latch above
        crate::pr_info!(
            "sched: kernel-preempt enabled (first IRQ-exit switch, cpu={})",
            crate::arch::cpu_id()
        );
    }
    crate::sched::schedule();
}
/// Shared timer-tick body (used by both the LAPIC-timer vector and the
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-50): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn handle_timer_tick(regs: &mut PtRegs, cpu: usize) {
    crate::arch::arch_floor::x86_64::trap::handle_timer_tick(regs, cpu)
}

/// Handle timer interrupt on the legacy PIC IRQ0 vector (virtual-wire
/// fallback; the LAPIC timer vector routes to `handle_timer_tick`
/// directly with a LAPIC EOI).
pub(crate) fn handle_timer_interrupt(regs: &mut PtRegs, cpu: usize) {
    // SP-5 r6 freeze ring: the service heartbeat entry (the following
    // set_next_trigger re-arm lands as its own TIMER_OP_ONESHOT entry).
    crate::drivers::intc::apic::timer_event_record(
        cpu,
        crate::drivers::intc::apic::TIMER_OP_SERVICE,
        0,
        0,
    );
    // EOI FIRST (early-Linux discipline): the handler below can call
    // schedule(); with the ISR bit still set the PIC would block every
    // same/lower-priority line — the timer included — until the task we
    // switched away from happens to resume.
    pic_send_eoi(IRQ_TIMER);
    // With the local APIC enabled, PIC lines arrive through LINT0 and
    // the APIC needs its EOI as well or delivery wedges.
    crate::drivers::intc::apic::eoi();

    // Re-arm the PIT (no-op while it free-runs).
    crate::drivers::timer::set_next_trigger();

    handle_timer_tick(regs, cpu);
}

/// Handle an external IRQ (vectors 33..47 / spurious catch-all).
///
/// PIC analogue of the twin's PLIC flow: claim the highest-priority
/// in-service line, dispatch through the line table, then a conservative
/// non-specific EOI (early-Linux style — unconditional, idempotent at
/// the chip, and correct even for spurious IRQ7/IRQ15 storms).
pub(crate) fn handle_external_interrupt(vector: u64) {
    // The timer line never reaches here (vector 32 has its own gate and
    // EOI-first handler), so every claimed line is a device line.
    // RT-plan R5: under forced threading the top half only MASKS the
    // claimed line (the reference's mask_ack shape — suppression without
    // an open in-service window) and defers handler + unmask to the
    // per-line SCHED_FIFO 50 thread; the EOIs still happen here so the
    // LAPIC class never blocks and other lines keep delivering.
    let threaded = crate::interrupt::irqdesc::force_threaded();
    let mut claimed = 0;
    while let Some(line) = pic_claim() {
        let h = IRQ_LINE_HANDLERS[line as usize].load(core::sync::atomic::Ordering::Acquire);
        if h != 0 {
            if threaded {
                // Suppress re-fire while the thread is pending, then
                // defer. The thread runs the action (with the
                // irq_handler_entry/exit tracepoints — the reference
                // brackets the action where it executes, which for a
                // threaded IRQ is the thread) and unmasks on completion.
                mask_irq_line(line);
                if !INTX_IRQ_THREADS[line as usize].wake() {
                    // Defensive: registration raced (cannot happen via
                    // request_irq_line_named's spawn-before-unmask
                    // ordering) — run inline this once and re-arm.
                    run_line_action(line, h);
                    unmask_irq_line(line);
                }
            } else {
                run_line_action(line, h);
            }
        } else {
            // Once per line per boot: on the OH q35 boot, spurious PIC
            // lines (2 via vectors 0x2a/0x2b) fire continuously; the
            // per-IRQ print line was a measurable share of the serial log.
            static LINES_SEEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
            let seen = LINES_SEEN.fetch_or(1u32 << line, core::sync::atomic::Ordering::Relaxed);
            if seen & (1u32 << line) == 0 {
                crate::pr_debug!(
                    "trap: unhandled IRQ line {} (vector {:#x}) [once per line]",
                    line,
                    vector
                );
            }
        }
        pic_send_eoi(line);
        // PIC lines arrive through LINT0 (virtual wire) when the local
        // APIC is enabled: the APIC EOI must pair with the PIC EOI.
        crate::drivers::intc::apic::eoi();
        claimed += 1;
        if claimed >= 16 {
            break; // storm guard
        }
    }
    if claimed == 0 {
        // Spurious (or an unexpected vector >= 48): nothing is in
        // service, so deliberately NO EOI (EOI-ing a spurious IRQ7/15
        // confuses the PIC's in-service tracking).
        crate::pr_debug!("trap: spurious IRQ (vector {:#x})", vector);
    }
}

/// Handle system call — x86 port of the twin's handle_syscall.
pub(crate) fn handle_syscall(regs: &mut PtRegs) {
    // forkprof trace: first Rust code of a syscall (entry side). The
    // nr rides a second event (site 15, pid field = nr) so the timeline
    // names each child-side call.
    if crate::dfx::forkprof::trace_on() {
        crate::dfx::forkprof::trace(14, crate::sched::get_current_pid()); // T_SYSENTRY
        crate::dfx::forkprof::trace(15, regs.rax as u32); // T_SYSNR
    }
    // Re-enable interrupts before entering the syscall handler.  SYSCALL
    // entered with IF=0 (FMASK cleared it); running the whole syscall
    // with IF=0 starves timer ticks, softlockup detection, and preemption
    // — the same reasoning as the twin's local_irq_enable.  The saved
    // user rflags in the frame is untouched, so iretq restores the user's
    // IF on return.
    crate::arch::cpu::enable_irq();

    let syscall_num = regs.rax; // nr in rax; args are rdi rsi rdx r10 r8 r9

    // Default return value is -ENOSYS (dispatch overrides on success).
    regs.rax = -(crate::errno::constants::ENOSYS as i64) as u64;

    // No rip adjustment: SYSCALL already moved the user rip into RCX and
    // the stub stored it in the frame's rip slot.

    // Seccomp gate before any dispatch (U1c; same as the twin).
    if let Some(rv) = crate::syscall::process::seccomp_syscall_gate(syscall_num, regs) {
        regs.rax = rv as u64;
        return;
    }

    crate::syscall::syscall_handler(regs);

}
/// Handle illegal instruction (#UD).
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-49): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn handle_illegal_instruction(regs: &mut PtRegs) {
    crate::arch::arch_floor::x86_64::trap::handle_illegal_instruction(regs)
}

/// SIGILL (4) disposition check for the #UD path (processdump/CFI crash
/// catchers in OH install one).
pub(crate) fn sigill_has_handler() -> bool {
    crate::sched::current()
        .and_then(|t| (*t).signal.as_ref().map(|s| s.get_action(4)))
        .map(|a| a.is_some_and(|a| a.has_handler()))
        .unwrap_or(false)
}

/// Handle #GP: user mode → SIGSEGV (Linux semantics), kernel → bug.
pub(crate) fn handle_general_protection(regs: &mut PtRegs) {
    // OH bring-up: name the faulting task — b85's repeating user #GP at
    // ld.so+0xba256 could not be attributed to any pid.
    // HOTPATH-PRINT: budget-gated (8/boot) — user #GP fires per
    // protection fault; the SIGSEGV delivery is the event.
    static GP_N: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    if GP_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
        crate::pr_info!("trap: #GP at rip={:#x}, mode={}, pid {}",
            regs.rip, if regs.user_mode() { "user" } else { "kernel" },
            crate::process::current_pid());
    }

    if regs.user_mode() {
        // hp88 hunt (TASK 87.11): during hiperf record sampling processes
        // die at ld-musl+0xba256 (mallocng get_meta consistency check —
        // a non-canonical heap metadata pointer). The #PF path has rich
        // forensics (user_segv_forensics); #GP had only this one line, so
        // the corrupted metadata's CONTENT was never captured. Reuse the
        // full report (bounded: 6 full dumps then one-liners) plus a
        // register-memory dump — perf-record-shaped bytes in the heap
        // metadata would name the ring writer as the corrupting agent.
        user_segv_forensics(regs, regs.rip);
        gp_regmem_dump(regs);
        gp_mmhistory_dump(regs);
        // Handler-first routing (same as the page-fault Segfault path):
        // debuggers/crash catchers must see the SIGSEGV; the default
        // disposition kills here. A fault inside the (blocked) SIGSEGV
        // handler itself is forced to the default action — unblocking
        // alone would re-enter the handler and loop forever.
        // si_code/si_addr parity (arena-mmap lane): #GP from user mode is
        // force_sig(SIGSEGV) — SI_KERNEL with si_addr 0 on the reference.
        let pid = crate::process::current_pid();
        let gp_info = crate::signal::SigInfo::fault(
            crate::signal::Signal::SIGSEGV as i32,
            crate::signal::si_code::SI_KERNEL,
            0,
        );
        if sigsegv_has_handler() && !sigsegv_blocked_current() {
            let _ = crate::signal::send_signal_with_info_directed(pid, gp_info);
            // Synchronous fault: deliver despite a blocked mask
            // (Linux force_sig parity — see force_unblock_current).
            crate::signal::force_unblock_current(crate::signal::Signal::SIGSEGV as i32);
            return;
        }
        let _ = crate::signal::send_signal_with_info_directed(pid, gp_info);
        // huks-wedge lane (hw5): group death (Linux do_group_exit).
        crate::process::exit::do_exit_group(-(crate::signal::Signal::SIGSEGV as i32));
    } else {
        #[cfg(feature = "x86_64")]
        crate::dfx::scribble::crash_report(b"#GP", regs);
        panic!(
            "trap: #GP in kernel mode at rip={:#x}, error={:#x}",
            regs.rip, regs.orig_rax
        );
    }
}

/// Arithmetic exceptions: #DE / #OF / #BR / #MF / #XM → SIGFPE.
pub(crate) fn handle_arith_exception(regs: &mut PtRegs, vector: u64) {
    crate::pr_debug!("trap: arith exception {:#x} at rip={:#x}", vector, regs.rip);
    if regs.user_mode() {
        // Linux delivers SIGFPE (catchable — fenv/overflow tests install
        // handlers); only the default disposition kills the task.
        let pid = crate::process::current_pid();
        let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGFPE as i32);
        if !sigfpe_has_handler() {
            // huks-wedge lane (hw5): group death (Linux do_group_exit).
            crate::process::exit::do_exit_group(-(crate::signal::Signal::SIGFPE as i32));
        }
    } else {
        panic!("trap: arith exception {:#x} in kernel mode at rip={:#x}", vector, regs.rip);
    }
}

/// #AC (alignment check) → SIGBUS in user mode.
pub(crate) fn handle_alignment_check(regs: &mut PtRegs) {
    if regs.user_mode() {
        let pid = crate::process::current_pid();
        let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGBUS as i32);
        if !sigbus_has_handler() {
            // huks-wedge lane (hw5): group death (Linux do_group_exit).
            crate::process::exit::do_exit_group(-(crate::signal::Signal::SIGBUS as i32));
        }
    } else {
        panic!("trap: #AC in kernel mode at rip={:#x}", regs.rip);
    }
}
/// #NM — FPU unavailable: the LAZY-FPU restore point.
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-48): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn handle_device_not_available() {
    crate::arch::arch_floor::x86_64::trap::handle_device_not_available()
}
/// #DB — hardware single-step (TF) or debug-register trap.
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-47): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn handle_debug(regs: &mut PtRegs) {
    crate::arch::arch_floor::x86_64::trap::handle_debug(regs)
}

/// SIGTRAP (5) disposition check for the #DB path (step tracers and
/// coverage tools install one; the default disposition kills).
pub(crate) fn sigtrap_has_handler() -> bool {
    crate::sched::current()
        .and_then(|t| (*t).signal.as_ref().map(|s| s.get_action(5)))
        .map(|a| a.is_some_and(|a| a.has_handler()))
        .unwrap_or(false)
}

/// True when the CURRENT task blocks SIGTRAP (bit 3) — the thread is
/// inside a SIGTRAP handler without SA_NODEFER (a trap-in-handler
/// loop would otherwise refault forever).
pub(crate) fn sigtrap_blocked_current() -> bool {
    crate::sched::current()
        .map(|t| t.sigmask & (1u64 << 3) != 0)
        .unwrap_or(false)
}
/// int3 (#BP) — port of the twin's handle_breakpoint for the int3
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-46): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn handle_breakpoint(regs: &mut PtRegs) {
    crate::arch::arch_floor::x86_64::trap::handle_breakpoint(regs)
}

// ============================================================================
// Page fault
// ============================================================================

/// True when the CURRENT task has a user handler installed for SIGSEGV
/// (port of the twin's helper — decides pend-vs-kill routing).
pub(crate) fn sigsegv_has_handler() -> bool {
    crate::sched::current()
        .and_then(|t| (*t).signal.as_ref().map(|s| s.get_action(11)))
        .map(|a| a.is_some_and(|a| a.has_handler()))
        .unwrap_or(false)
}

/// True when the CURRENT task blocks SIGSEGV (bit 10) — the thread is
/// inside a SIGSEGV handler (delivery blocks the signal unless
/// SA_NODEFER) or blocked it via sigprocmask. Linux force_sig_info
/// semantics: a FORCED signal that is blocked is reset to SIG_DFL and
/// unblocked — i.e. a synchronous fault on a blocked SIGSEGV kills the
/// task. Merely pending it and iretq-ing back to the faulting rip
/// refaults forever: OH's DFX_SignalHandler itself faults reading past
/// its thread-stack top, and the redelivery loop starved the whole
/// machine (96k+ identical faults in one boot).
pub(crate) fn sigsegv_blocked_current() -> bool {
    crate::sched::current()
        .map(|t| (*t).sigmask & (1u64 << 10) != 0)
        .unwrap_or(false)
}

pub(crate) fn sigbus_has_handler() -> bool {
    crate::sched::current()
        .and_then(|t| (*t).signal.as_ref().map(|s| s.get_action(7)))
        .map(|a| a.is_some_and(|a| a.has_handler()))
        .unwrap_or(false)
}

/// SIGFPE (8) disposition check for the arith-exception path.
pub(crate) fn sigfpe_has_handler() -> bool {
    crate::sched::current()
        .and_then(|t| (*t).signal.as_ref().map(|s| s.get_action(8)))
        .map(|a| a.is_some_and(|a| a.has_handler()))
        .unwrap_or(false)
}

/// #GP register-memory dump (hp88 hunt): two u64 words at every
/// canonical-user pointer register, read through the checked walk. The
/// mallocng get_meta #GP dereferences a metadata pointer read from the
/// heap — this shows what the surrounding heap words contain (perf-
/// record-shaped bytes would name the corrupting writer directly).
// HOTPATH-PRINT (whole fn): fatal-path forensics — called once from
// the #GP death report, bounded by the task's exit.
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-45): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn gp_regmem_dump(regs: &PtRegs) {
    crate::arch::arch_floor::x86_64::trap::gp_regmem_dump(regs)
}
/// hp89 hunt: the victim tgid's mm-syscall history at user-#GP time.
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-44): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn gp_mmhistory_dump(regs: &PtRegs) {
    crate::arch::arch_floor::x86_64::trap::gp_mmhistory_dump(regs)
}

// huks-wedge lane (hw2): SEGV-history ring — (tid, fault jiffies) for
// the last 64 delivered user SIGSEGVs. The holder-exit print in do_exit
// reads it: a thread that took a HANDLED SIGSEGV (FFRT's arena/stack
// handler consumed it) and then exits "cleanly" is the wedge's first
// face (hw4 tid 477: boundary-SEGV at 9.0 s, thread gone by 34 s with
// the process alive), and without this ring the exit looks voluntary.
pub(crate) static SEGV_HIST_TID: [core::sync::atomic::AtomicU32; 64] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 64];
pub(crate) static SEGV_HIST_J: [core::sync::atomic::AtomicU64; 64] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 64];
pub(crate) static SEGV_HIST_IX: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

pub(crate) fn segv_hist_note(fault_addr: u64) {
    let tid = crate::sched::get_current_pid() as u32;
    if tid == 0 {
        return;
    }
    let i = SEGV_HIST_IX.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % 64;
    SEGV_HIST_TID[i].store(tid, core::sync::atomic::Ordering::Relaxed);
    // Upper 32 bits carry cr2's low 32 (page id for correlation); lower
    // 32 carry jiffies (age at exit-print time).
    SEGV_HIST_J[i].store(
        ((fault_addr & 0xffff_ffff) << 32) | (crate::drivers::timer::get_jiffies() & 0xffff_ffff),
        core::sync::atomic::Ordering::Relaxed,
    );
}

/// Age (in jiffies) of this tid's most recent delivered SIGSEGV, plus
/// that fault's cr2, if the tid ever took one.
pub fn segv_hist_age(tid: u32) -> Option<(u64, u64)> {
    let now = crate::drivers::timer::get_jiffies();
    let mut best_j = 0u64;
    let mut best_addr = 0u64;
    let mut found = false;
    for i in 0..64 {
        if SEGV_HIST_TID[i].load(core::sync::atomic::Ordering::Relaxed) == tid {
            let w = SEGV_HIST_J[i].load(core::sync::atomic::Ordering::Relaxed);
            let j = w & 0xffff_ffff;
            if !found || j >= best_j {
                found = true;
                best_j = j;
                best_addr = w >> 32;
            }
        }
    }
    if found {
        Some((now.saturating_sub(best_j), best_addr))
    } else {
        None
    }
}

/// Fatal user-mode SIGSEGV forensics: the GP register set, the VMA
/// covering the faulting rip, and the first stack qwords (return
/// addresses) — enough to identify which mapping jumped through a
/// garbage pointer (the gnome-shell ELF-magic wild-jump class:
/// fault == 0x010102464c457f == LE bytes "\x7fELF\x02\x01\x01\x00").
/// Diagnostic only: every read is checked through the page-table walk.
pub(crate) static USEGV_FOR_ENTERED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
// HOTPATH-PRINT (whole fn): fatal-path forensics — one dump per user
// SIGSEGV death; the sending task exits immediately after.
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-43): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn user_segv_forensics(regs: &PtRegs, fault_addr: u64) {
    crate::arch::arch_floor::x86_64::trap::user_segv_forensics(regs, fault_addr)
}

/// Handle page fault (#PF): decode the error code (in the orig_rax slot)
/// into FaultFlags and hand off to `exception::do_page_fault`, then
/// mirror the twin's outcome routing.
/// R73: one-shot latch for the KERNPANIC raw dump. A fault DURING the
/// dump (stack walk past the mapping, scribble park hook, frame walk on
/// a wild rbp) re-enters `handle_page_fault`; without the latch that
/// recursion prints the dump again and faults again — the print/read
/// loop that wedged sc195's cpu0 for ~855 s (hardirq-held, rip parked
/// in the UART `inb`). Second entry prints one marker and halts.
pub(crate) static KP_DUMP_ENTERED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-42): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn handle_page_fault(regs: &mut PtRegs) {
    crate::arch::arch_floor::x86_64::trap::handle_page_fault(regs)
}

/// The oops register block — the FAULTING frame in the reference's
/// show_regs layout (arch/x86/kernel/process_64.c + fault.c, Linux
/// 5.15): the RIP line with the symbolized ip, the RSP/EFLAGS line,
/// then the GPRs three per row. The CS column prints 0010/0018 from
/// the frame's segment selectors.
pub(crate) fn oops_show_regs(w: &mut crate::dfx::backtrace::ConsoleWriter, regs: &PtRegs) {
    use core::fmt::Write;
    let _ = write!(
        w,
        "RIP: {:04x}:{}\n",
        regs.cs & 0xffff,
        crate::dfx::ksyms::format_symbol(regs.rip)
    );
    let _ = write!(
        w,
        "RSP: {:04x}:{:016x} EFLAGS: {:016x}\n",
        regs.ss & 0xffff,
        regs.rsp,
        regs.rflags
    );
    let cells: [(&str, u64); 15] = [
        ("RAX", regs.rax),
        ("RBX", regs.rbx),
        ("RCX", regs.rcx),
        ("RDX", regs.rdx),
        ("RSI", regs.rsi),
        ("RDI", regs.rdi),
        ("RBP", regs.rbp),
        ("R8", regs.r8),
        ("R9", regs.r9),
        ("R10", regs.r10),
        ("R11", regs.r11),
        ("R12", regs.r12),
        ("R13", regs.r13),
        ("R14", regs.r14),
        ("R15", regs.r15),
    ];
    for row in cells.chunks(3) {
        for (i, (name, val)) in row.iter().enumerate() {
            if i > 0 {
                let _ = w.write_str(" ");
            }
            let _ = write!(w, "{:<3}: {:016x}", name, val);
        }
        let _ = w.write_str("\n");
    }
}

/// Handle unknown exception (port of the twin's handler).
pub(crate) fn handle_unknown_exception(regs: &mut PtRegs, vector: u64) {
    // HOTPATH-PRINT (whole fn): fatal-path forensics — one report per
    // unknown-vector trap, then the task dies (no repeat per process).
    let cause = Cause::from_cause(vector);
    // HOTPATH-PRINT: fatal-path — once per unknown-vector trap (task dies).
    crate::pr_err!(
        "trap: Unknown exception {:?} (vector {:#x}), rip={:#x}",
        cause,
        vector,
        regs.rip
    );

    if regs.user_mode() {
        crate::process::exit::do_exit(-(crate::signal::Signal::SIGKILL as i32));
    } else {
        // Kernel-mode unknown exceptions indicate a kernel bug; skipping
        // the instruction corrupts state. The Oops path prints the
        // report and applies the policy (Tranche 5).
        crate::dfx::oops::oops(
            crate::dfx::oops::Kind::UnknownException { cause: vector },
            &|w: &mut crate::dfx::backtrace::ConsoleWriter| {
                oops_show_regs(w, regs);
            },
        );
    }
}

// ============================================================================
// asm-callable helpers
// ============================================================================




// ============================================================================
// sc105: stime-burn attribution histogram
// ============================================================================
//
// The OH launcher child spends its whole 3 s appspawn watchdog life in
// kernel time (utime 0-2t, stime 183-190t, on_cpu). stime is tick-sampled
// accounting — so sample WHERE each kernel-mode tick lands. Each slot keys
// on (task comm, 4 KiB kernel-text page) so one task's burn is separable
// from the ~580-task boot crowd; KILLDUMP (kill_dump_wedge) prints the
// table when appspawn's watchdog kill fires.
//
// Fixed-size, no allocation, IRQ-context safe; lost updates on the
// steal-smallest path are fine (a probe, not an accounting device).

pub(crate) const STIME_SLOTS: usize = 24;

pub(crate) struct StimeSample {
    pub(crate) comm: [core::sync::atomic::AtomicU8; 16],
    pub(crate) page: core::sync::atomic::AtomicU64,
    pub(crate) count: core::sync::atomic::AtomicU64,
}

pub(crate) static STIME_HIST: [StimeSample; STIME_SLOTS] = {
    const EMPTY: StimeSample = StimeSample {
        comm: [const { core::sync::atomic::AtomicU8::new(0) }; 16],
        page: core::sync::atomic::AtomicU64::new(0),
        count: core::sync::atomic::AtomicU64::new(0),
    };
    [EMPTY; STIME_SLOTS]
};

/// Focus table (sc106): launcher-comm samples only — see stime_sample_rip.
pub(crate) static STIME_FOCUS: [StimeSample; STIME_SLOTS] = {
    const EMPTY: StimeSample = StimeSample {
        comm: [const { core::sync::atomic::AtomicU8::new(0) }; 16],
        page: core::sync::atomic::AtomicU64::new(0),
        count: core::sync::atomic::AtomicU64::new(0),
    };
    [EMPTY; STIME_SLOTS]
};

// ==================== leg-40C perf rider: user-mode rip sampling ====================
// The cold-start wall is compute-bound ArkTS work in the launcher's
// user space; stime_sample_rip only sees kernel-mode ticks. These
// tables mirror the pattern for USER-mode rips (module attribution
// offline via the byte-matched .so inventory — launcher mmaps are
// deterministic, no ASLR), plus raw utime/stime tick counts for the
// mode split. Printed every 60 s by utime_ripsample_report (called
// from the 1 Hz drift block in the tick path, dfx-gated).

pub(crate) const UTIME_SLOTS: usize = 32;

pub(crate) struct UtimeSample {
    pub(crate) page: core::sync::atomic::AtomicU64,
    pub(crate) count: core::sync::atomic::AtomicU64,
}

pub(crate) static UTIME_HIST: [UtimeSample; UTIME_SLOTS] = {
    const EMPTY: UtimeSample = UtimeSample {
        page: core::sync::atomic::AtomicU64::new(0),
        count: core::sync::atomic::AtomicU64::new(0),
    };
    [EMPTY; UTIME_SLOTS]
};

/// launcher-comm user ticks (mode split numerator).
pub(crate) static UTIME_LAUNCHER_TICKS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
/// launcher-comm kernel ticks.
pub(crate) static STIME_LAUNCHER_TICKS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

pub(crate) fn comm_has_launcher(comm: &[u8; 16]) -> bool {
    let mut run = 0u8;
    for &b in comm.iter() {
        if b == 0 {
            break;
        }
        run = if b == b"launcher"[run as usize] {
            run + 1
        } else if b == b"launcher"[0] {
            1
        } else {
            0
        };
        if run as usize == b"launcher".len() {
            return true;
        }
    }
    false
}

/// Record one USER-mode tick at `rip` for a launcher-comm task.
pub(crate) fn utime_sample_rip(rip: u64, comm: &[u8; 16]) {
    use core::sync::atomic::Ordering::Relaxed;
    if !comm_has_launcher(comm) {
        return;
    }
    UTIME_LAUNCHER_TICKS.fetch_add(1, Relaxed);
    let page = rip & !0xFFFu64;
    let mut min_idx = 0usize;
    let mut min_count = u64::MAX;
    for i in 0..UTIME_SLOTS {
        let p = UTIME_HIST[i].page.load(Relaxed);
        if p == 0 {
            if UTIME_HIST[i]
                .page
                .compare_exchange(
                    0,
                    page,
                    core::sync::atomic::Ordering::AcqRel,
                    core::sync::atomic::Ordering::Acquire,
                )
                .is_ok()
            {
                UTIME_HIST[i].count.store(1, Relaxed);
                return;
            }
            continue;
        }
        if p == page {
            UTIME_HIST[i].count.fetch_add(1, Relaxed);
            return;
        }
        let c = UTIME_HIST[i].count.load(Relaxed);
        if c < min_count {
            min_count = c;
            min_idx = i;
        }
    }
    if UTIME_HIST[min_idx]
        .count
        .compare_exchange(
            min_count,
            1,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Acquire,
        )
        .is_ok()
    {
        UTIME_HIST[min_idx].page.store(page, Relaxed);
    }
}

/// Periodic report (rux:ripsample (retired dfx=ripsample)): the launcher's user-rip histogram
/// + utime/stime split + the kernel-mode focus table. Self-throttled
/// to one print every 60 s.
pub // HOTPATH-PRINT (whole fn): one-shot diagnostic report (armed by the
// ripsample boot param; prints once at report time).
fn utime_ripsample_report() {
    use core::sync::atomic::Ordering::Relaxed;
    if !crate::trace::EVENT_RIPSAMPLE.armed() {
        return;
    }
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let j = crate::drivers::timer::x86_64::get_jiffies();
    if j < LAST.load(Relaxed) + 60 * crate::config::KERNEL_HZ as u64 {
        return;
    }
    LAST.store(j, Relaxed);
    let u = UTIME_LAUNCHER_TICKS.load(Relaxed);
    let s = STIME_LAUNCHER_TICKS.load(Relaxed);
    // HOTPATH-PRINT: one-shot report (armed by ripsample boot param)
    crate::pr_warn!("RIPSAMPLE: launcher utime={} stime={}", u, s);
    for i in 0..UTIME_SLOTS {
        let p = UTIME_HIST[i].page.load(Relaxed);
        let c = UTIME_HIST[i].count.load(Relaxed);
        if p != 0 && c > 0 {
            // HOTPATH-PRINT: one-shot report (armed by ripsample boot param)
            crate::pr_warn!("RIPSAMPLE: u-page={:#x} n={}", p, c);
        }
    }
    for i in 0..STIME_SLOTS {
        let p = STIME_FOCUS[i].page.load(Relaxed);
        let c = STIME_FOCUS[i].count.load(Relaxed);
        if p != 0 && c > 0 {
            let mut comm = [0u8; 16];
            for jx in 0..16 {
                comm[jx] = STIME_FOCUS[i].comm[jx].load(Relaxed);
            }
            let name = core::str::from_utf8(&comm[..]).unwrap_or("").trim_end_matches('\0');
            // HOTPATH-PRINT: one-shot report (armed by ripsample boot param)
            crate::pr_warn!("RIPSAMPLE: k-page={:#x} n={} comm={}", p, c, name);
        }
    }
}

/// Record one kernel-mode tick at `rip` for `comm`.
pub(crate) fn stime_sample_rip(rip: u64, comm: &[u8; 16]) {
    use core::sync::atomic::Ordering::{Acquire, Relaxed};
    // Only kernel text addresses (user rip cannot be a stime sample, but
    // vmalloc/modules would bucket misleadingly).
    let (lo, hi) = crate::sched::sched::kernel_text_window();
    if rip < lo || rip >= hi {
        return;
    }
    let page = rip & !0xFFFu64;

    // sc106 lesson: the global steal-smallest table starves the very task
    // under investigation — the launcher child burns ~180 ticks in 3 s
    // bursts, but every fresh (comm,page) entry starts at 1 and is stolen
    // away by the next new page before it accumulates. Focus table: comm
    // containing "launcher" (the killed children run comm='m.ohos.
    // launcher', set post-exec by appspawn) never competes with the boot
    // crowd.
    let focus = {
        let mut hit = false;
        let mut run = 0u8;
        for &b in comm.iter() {
            if b == 0 {
                break;
            }
            run = if b == b"launcher"[run as usize] {
                run + 1
            } else if b == b"launcher"[0] {
                1
            } else {
                0
            };
            if run as usize == b"launcher".len() {
                hit = true;
                break;
            }
        }
        hit
    };
    let hist: &[StimeSample; STIME_SLOTS] = if focus {
        &STIME_FOCUS
    } else {
        &STIME_HIST
    };

    let mut min_idx = 0usize;
    let mut min_count = u64::MAX;
    for i in 0..STIME_SLOTS {
        let p = hist[i].page.load(Relaxed);
        if p == 0 {
            // Claim the empty slot (page first — count follows only after
            // the page store, so readers never see a stale count against
            // a new page unless they load between the two, which the
            // count reset below makes benign for a probe).
            if hist[i]
                .page
                .compare_exchange(0, page, core::sync::atomic::Ordering::AcqRel, Acquire)
                .is_ok()
            {
                for j in 0..16 {
                    hist[i].comm[j].store(comm[j], Relaxed);
                }
                hist[i].count.store(1, Relaxed);
                return;
            }
            continue;
        }
        if p == page {
            // Same page; compare comm to keep tasks separable.
            let same = (0..16).all(|j| hist[i].comm[j].load(Relaxed) == comm[j]);
            if same {
                hist[i].count.fetch_add(1, Relaxed);
                return;
            }
        }
        let c = hist[i].count.load(Relaxed);
        if c < min_count {
            min_count = c;
            min_idx = i;
        }
    }
    // Table full and this (comm, page) has no slot: replace the least
    // sampled one. Single writer wins via a count swap; the loser's
    // sample is dropped.
    if hist[min_idx]
        .count
        .compare_exchange(min_count, 1, core::sync::atomic::Ordering::AcqRel, Acquire)
        .is_ok()
    {
        hist[min_idx].page.store(page, Relaxed);
        for j in 0..16 {
            hist[min_idx].comm[j].store(comm[j], Relaxed);
        }
    }
}

/// Snapshot the stime histogram for KILLDUMP: (comm, page, ticks) rows,
/// highest count first, empty slots omitted. `focus=true` returns the
/// launcher-gated table instead of the global one.
pub fn stime_profile_top(focus: bool) -> alloc::vec::Vec<([u8; 16], u64, u64)> {
    use core::sync::atomic::Ordering::Relaxed;
    let hist: &[StimeSample; STIME_SLOTS] = if focus {
        &STIME_FOCUS
    } else {
        &STIME_HIST
    };
    let mut v = alloc::vec::Vec::new();
    for i in 0..STIME_SLOTS {
        let page = hist[i].page.load(Relaxed);
        if page == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = hist[i].comm[j].load(Relaxed);
        }
        v.push((comm, page, hist[i].count.load(Relaxed)));
    }
    v.sort_by(|a, b| b.2.cmp(&a.2));
    v
}

// Collected-unsafe re-exports: the unsafe floor for these names lives in
// kernel/src/arch/unsafe.rs (INV-ARCH-*); these keep the original paths.
// INV-ARCH note: ret_from_fork_kernel_helper moved to arch::arch_floor::x86_64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::x86_64::trap::ret_from_fork_kernel_helper;
// INV-ARCH note: x86_double_fault_dump moved to arch::arch_floor::x86_64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::x86_64::trap::x86_double_fault_dump;
// INV-ARCH note: x86_preemptible moved to arch::arch_floor::x86_64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::x86_64::trap::x86_preemptible;
// INV-ARCH note: trap_handler moved to arch::arch_floor::x86_64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::x86_64::trap::trap_handler;
// INV-ARCH note: x86_bad_frame_exit moved to arch::arch_floor::x86_64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::x86_64::trap::x86_bad_frame_exit;
pub(crate) use crate::arch::arch_floor::x86_64::trap::DOUBLE_FAULT_STACKS;
// INV-ARCH note: ret_from_fork_kernel moved to arch::arch_floor::x86_64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::x86_64::trap::ret_from_fork_kernel;
// INV-ARCH note: ret_from_fork moved to arch::arch_floor::x86_64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::x86_64::trap::ret_from_fork;
