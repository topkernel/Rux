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
//! STAR = (0x20<<48) | (0x08<<32): SYSCALL enters on CS=0x08/SS=0x10;
//! SYSRET64 returns to CS = STAR[63:48]+16 = 0x33, SS = +8 = 0x2b.
//! FMASK = TF|IF|DF — syscalls enter with IF clear; `handle_syscall`
//! re-enables interrupts like the riscv64 twin.  Return is via iretq
//! (never sysretq): avoids the SYSRET canonical-RSP crash class and
//! keeps one uniform exit path.

use super::cpu::{inb, outb, rdmsr, wrmsr};
use crate::arch::pt_regs::{Cause, PtRegs, PT_REGS_SIZE};

// Include trap.S assembly code (AT&T syntax — global_asm defaults to
// Intel on x86 targets).
core::arch::global_asm!(include_str!("trap.S"), options(att_syntax));

// Table-load helpers (Intel-syntax inline quirks make the reload dance
// easier as one asm unit: lgdt + CS reload + segment reloads + ltr).
core::arch::global_asm!(
    r#"
.global x86_load_gdt
.hidden x86_load_gdt
x86_load_gdt:                  # rdi = &PseudoDescriptor
    lgdt (%rdi)
    pushq $0x08                # __KERNEL_CS
    leaq 1f(%rip), %rax
    pushq %rax
    lretq                      # reload CS from the new GDT
1:  movw $0x10, %ax            # __KERNEL_DS
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    xorl %eax, %eax
    movw %ax, %fs
    movw %ax, %gs
    movw $0x38, %ax            # TSS selector
    ltr %ax
    ret

.global x86_load_idt
.hidden x86_load_idt
x86_load_idt:                  # rdi = &PseudoDescriptor
    lidt (%rdi)
    ret
"#,
    options(att_syntax)
);

// ============================================================================
// Static asserts: pin the pt_regs layout trap.S was written against
// ============================================================================

const _: () = assert!(PT_REGS_SIZE == 0xa8);
const _: () = assert!(core::mem::offset_of!(PtRegs, r15) == 0x00);
const _: () = assert!(core::mem::offset_of!(PtRegs, r14) == 0x08);
const _: () = assert!(core::mem::offset_of!(PtRegs, r13) == 0x10);
const _: () = assert!(core::mem::offset_of!(PtRegs, r12) == 0x18);
const _: () = assert!(core::mem::offset_of!(PtRegs, rbp) == 0x20);
const _: () = assert!(core::mem::offset_of!(PtRegs, rbx) == 0x28);
const _: () = assert!(core::mem::offset_of!(PtRegs, r11) == 0x30);
const _: () = assert!(core::mem::offset_of!(PtRegs, r10) == 0x38);
const _: () = assert!(core::mem::offset_of!(PtRegs, r9) == 0x40);
const _: () = assert!(core::mem::offset_of!(PtRegs, r8) == 0x48);
const _: () = assert!(core::mem::offset_of!(PtRegs, rax) == 0x50);
const _: () = assert!(core::mem::offset_of!(PtRegs, rcx) == 0x58);
const _: () = assert!(core::mem::offset_of!(PtRegs, rdx) == 0x60);
const _: () = assert!(core::mem::offset_of!(PtRegs, rsi) == 0x68);
const _: () = assert!(core::mem::offset_of!(PtRegs, rdi) == 0x70);
const _: () = assert!(core::mem::offset_of!(PtRegs, orig_rax) == 0x78);
const _: () = assert!(core::mem::offset_of!(PtRegs, rip) == 0x80);
const _: () = assert!(core::mem::offset_of!(PtRegs, cs) == 0x88);
const _: () = assert!(core::mem::offset_of!(PtRegs, rflags) == 0x90);
const _: () = assert!(core::mem::offset_of!(PtRegs, rsp) == 0x98);
const _: () = assert!(core::mem::offset_of!(PtRegs, ss) == 0xa0);

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

const _: () = assert!(TSS_OFF_RSP0 == 4 && TSS_OFF_SP2 == 0x14 && TSS_OFF_IST1 == 0x24);

/// The syscall vector used between trap.S and trap_handler (not an IDT
/// entry — syscalls arrive through LSTAR).
pub const SYSCALL_VECTOR: u64 = 0x80;

/// STAR value: kernel CS 0x08 on SYSCALL; user base 0x20 so SYSRET64
/// computes CS = 0x20+16 = 0x33 (user code64), SS = 0x20+8 = 0x2b.
const MSR_STAR_VALUE: u64 = (0x20u64 << 48) | (0x08u64 << 32);
/// FMASK: TF(8) | IF(9) | DF(10) cleared on syscall entry.
const MSR_FMASK_VALUE: u64 = (1 << 8) | (1 << 9) | (1 << 10);

const MSR_EFER: u32 = 0xC000_0080;
const MSR_STAR: u32 = 0xC000_0081;
const MSR_LSTAR: u32 = 0xC000_0082;
const MSR_FMASK: u32 = 0xC000_0084;
const EFER_SCE: u64 = 1;
/// EFER.NXE — without it every PTE carrying NX (all non-executable user
/// mappings: stacks, data, bss) raises a reserved-bit #PF (ec bit 3) on
/// ANY access, which the dispatcher then misreads as a read fault and
/// "handles" into an infinite loop.
const EFER_NXE: u64 = 1 << 11;

// ============================================================================
// GDT / TSS / IDT tables
// ============================================================================

/// GDTR/LIDTR pseudo-descriptor (packed: 10 bytes).
#[repr(C, packed)]
struct PseudoDescriptor {
    limit: u16,
    base: u64,
}

// asm table-load helpers defined in the global_asm! block below.
extern "C" {
    fn x86_load_gdt(pd: *const PseudoDescriptor);
    fn x86_load_idt(pd: *const PseudoDescriptor);
}

/// 16-byte IDT gate descriptor (64-bit mode).
#[repr(C)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,      // bits 0..2 = IST index, rest 0
    type_attr: u8, // P<<7 | DPL<<5 | 0<<4 | gate type (0xE = interrupt)
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn empty() -> Self {
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

    fn set_handler(&mut self, addr: u64, ist: u8, dpl: u8) {
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
    bytes: [u8; TSS_SIZE],
}

impl Tss {
    const fn zeroed() -> Self {
        Tss { bytes: [0; TSS_SIZE] }
    }

    #[inline]
    fn write_u64(&mut self, off: usize, v: u64) {
        // SAFETY: off is a fixed field offset inside self; unaligned
        // 8-byte writes are architecturally fine on x86_64.
        unsafe { core::ptr::write_unaligned(self.bytes.as_mut_ptr().add(off) as *mut u64, v) };
    }

    #[inline]
    #[allow(dead_code)]
    fn read_u64(&self, off: usize) -> u64 {
        // SAFETY: off is a fixed field offset inside self.
        unsafe { core::ptr::read_unaligned(self.bytes.as_ptr().add(off) as *const u64) }
    }
}

/// Per-CPU TSS tables (the TSS referenced by each CPU's own GDT at
/// selector 0x38; IST1 = per-CPU double-fault stack, rsp0 = the current
/// task's kernel stack top).
static mut TSS_PER_CPU: [Tss; crate::config::MAX_CPUS] =
    [const { Tss::zeroed() }; crate::config::MAX_CPUS];

/// IST1 stack for the double-fault handler (16K per CPU, .bss.stack so
/// it sits with the other privileged stacks).
#[repr(align(16))]
struct DoubleFaultStack([u8; 16384]);

#[link_section = ".bss.stack"]
static mut DOUBLE_FAULT_STACKS: [DoubleFaultStack; crate::config::MAX_CPUS] =
    [const { DoubleFaultStack([0; 16384]) }; crate::config::MAX_CPUS];

/// GDT: null, code64, data, (2 unused), user data, user code64, TSS(16B).
#[repr(C, align(8))]
struct Gdt([u64; 9]);

/// Segment descriptor helper: base=0, limit=0xFFFF, flags in the top byte.
const fn seg_desc(access: u8, flags: u8) -> u64 {
    ((flags as u64) << 52) | ((access as u64) << 40) | 0xFFFF
}

/// Per-CPU GDTs (each points at that CPU's TSS through entries 7/8).
static mut GDT_PER_CPU: [Gdt; crate::config::MAX_CPUS] = [const {
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
static mut IDT_X86: [IdtEntry; 256] = [const { IdtEntry::empty() }; 256];

/// Update TSS.rsp0 (the kernel stack top user entries switch to) and
/// its %gs-readable mirror (syscall_entry).  Runs on the local CPU.
pub fn set_tss_rsp0(top: u64) {
    let cpu = crate::arch::cpu_id() as usize;
    // SAFETY: the TSS slot belongs to this CPU; the only racing context
    // (an entry reading rsp0) either sees the old valid stack or the new
    // one.
    unsafe {
        let tss = &raw mut TSS_PER_CPU;
        (*tss)[cpu].write_u64(TSS_OFF_RSP0, top);
    }
    super::smp::PER_CPU[cpu].tss_rsp0.store(top, core::sync::atomic::Ordering::Release);
}

// ============================================================================
// init
// ============================================================================

/// Build and load this CPU's GDT/TSS (same selector layout on every
/// CPU; the TSS descriptor points at THIS CPU's TSS), then re-assert
/// the GS base (lgdt loads a null GS selector; the architectural GS
/// base survives that, but re-writing it keeps the invariant explicit).
fn init_cpu_state(cpu: usize) {
    unsafe {
        // ---- TSS ----
        let tss = &raw mut TSS_PER_CPU;
        let ist1 = DOUBLE_FAULT_STACKS[cpu].0.as_ptr() as usize + DOUBLE_FAULT_STACKS[cpu].0.len();
        (*tss)[cpu].write_u64(TSS_OFF_IST1, ist1 as u64);
        (*tss)[cpu].write_u64(TSS_OFF_RSP0, 0); // no task kernel stack until the scheduler starts

        // ---- GDT: fill the TSS system descriptor (idx 7/8, sel 0x38) ----
        // System-descriptor base layout: base[23:0] at bits 16..39,
        // base[31:24] at bits 56..63, base[63:32] in the second word.
        // (Dropping base[31:24] truncates the .bss VMA
        // ffffffff80a53500 -> ffffffff00a53500; LTR then "works", but
        // the first interrupt delivery from user mode reads TSS.rsp0
        // from an unmapped page — #PF inside delivery, triple fault.)
        let base = &raw const (*tss)[cpu] as usize as u64;
        let limit = (TSS_SIZE - 1) as u64;
        let gdt = &raw mut GDT_PER_CPU;
        (*gdt)[cpu].0[7] = (limit & 0xFFFF)
            | ((base & 0x00FF_FFFF) << 16)
            | (0x89 << 40)
            | (((base >> 24) & 0xFF) << 56);
        (*gdt)[cpu].0[8] = base >> 32;

        let gdtr = PseudoDescriptor {
            limit: (core::mem::size_of::<Gdt>() - 1) as u16,
            base: &raw const (*gdt)[cpu] as usize as u64,
        };
        // SAFETY: x86_load_gdt lgdt's the table initialized above (a
        // static), reloads the segment registers and ltr's the TSS.
        x86_load_gdt(&gdtr);
    }
    super::smp::load_per_cpu_base(cpu);
}

/// Initialize trap handling on the boot CPU: FPU, per-CPU GDT/TSS, the
/// shared IDT, and the remapped PIC.  Called before `arch::mm::init()`,
/// so all tables live in kernel .data/.bss already mapped by the boot
/// page tables.
pub fn init() {
    unsafe {
        // Enable the FPU before anything can touch FP state (context
        // switches run fxsave/fxrstor unconditionally).
        // SAFETY: one-time boot-CPU feature setup.
        super::thread::fpu_init();

        init_cpu_state(0);

        // ---- IDT ----
        extern "C" {
            static stub_0: u8;
            static stub_1: u8;
            static stub_2: u8;
            static stub_3: u8;
            static stub_4: u8;
            static stub_5: u8;
            static stub_6: u8;
            static stub_7: u8;
            static stub_8: u8;
            static stub_9: u8;
            static stub_10: u8;
            static stub_11: u8;
            static stub_12: u8;
            static stub_13: u8;
            static stub_14: u8;
            static stub_15: u8;
            static stub_16: u8;
            static stub_17: u8;
            static stub_18: u8;
            static stub_19: u8;
            static stub_20: u8;
            static stub_21: u8;
            static stub_22: u8;
            static stub_23: u8;
            static stub_24: u8;
            static stub_25: u8;
            static stub_26: u8;
            static stub_27: u8;
            static stub_28: u8;
            static stub_29: u8;
            static stub_30: u8;
            static stub_31: u8;
            static stub_32: u8;
            static stub_33: u8;
            static stub_34: u8;
            static stub_35: u8;
            static stub_36: u8;
            static stub_37: u8;
            static stub_38: u8;
            static stub_39: u8;
            static stub_40: u8;
            static stub_41: u8;
            static stub_42: u8;
            static stub_43: u8;
            static stub_44: u8;
            static stub_45: u8;
            static stub_46: u8;
            static stub_47: u8;
            static stub_spurious: u8;
            static stub_224: u8;
            static stub_225: u8;
            static stub_226: u8;
            static stub_227: u8;
            static stub_228: u8;
            static stub_229: u8;
        }

        let stubs: [*const u8; 48] = [
            &raw const stub_0, &raw const stub_1, &raw const stub_2, &raw const stub_3,
            &raw const stub_4, &raw const stub_5, &raw const stub_6, &raw const stub_7,
            &raw const stub_8, &raw const stub_9, &raw const stub_10, &raw const stub_11,
            &raw const stub_12, &raw const stub_13, &raw const stub_14, &raw const stub_15,
            &raw const stub_16, &raw const stub_17, &raw const stub_18, &raw const stub_19,
            &raw const stub_20, &raw const stub_21, &raw const stub_22, &raw const stub_23,
            &raw const stub_24, &raw const stub_25, &raw const stub_26, &raw const stub_27,
            &raw const stub_28, &raw const stub_29, &raw const stub_30, &raw const stub_31,
            &raw const stub_32, &raw const stub_33, &raw const stub_34, &raw const stub_35,
            &raw const stub_36, &raw const stub_37, &raw const stub_38, &raw const stub_39,
            &raw const stub_40, &raw const stub_41, &raw const stub_42, &raw const stub_43,
            &raw const stub_44, &raw const stub_45, &raw const stub_46, &raw const stub_47,
        ];
        // LAPIC timer (0xE0) + IPI vectors 0xE1..0xE5 (see arch/x86_64/ipi.rs).
        let apic_stubs: [*const u8; 6] = [
            &raw const stub_224, &raw const stub_225, &raw const stub_226,
            &raw const stub_227, &raw const stub_228, &raw const stub_229,
        ];

        let idt = &raw mut IDT_X86;
        for (v, entry) in (*idt).iter_mut().enumerate() {
            // int1/int3/into are reachable from user mode (DPL 3); #DF
            // uses IST1; everything else is a plain kernel interrupt gate.
            let lv = crate::drivers::intc::apic::LOCAL_TIMER_VECTOR as usize;
            let stub: *const u8 = if v < 48 {
                stubs[v]
            } else if v >= lv && v <= crate::arch::ipi::IPI_LAST as usize {
                apic_stubs[v - lv]
            } else {
                &raw const stub_spurious
            };
            let dpl = if v == 1 || v == 3 || v == 4 { 3 } else { 0 };
            let ist = if v == 8 { 1 } else { 0 };
            entry.set_handler(stub as u64, ist, dpl);
        }

        let idtr = PseudoDescriptor {
            limit: (core::mem::size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: idt as usize as u64,
        };
        // SAFETY: lidt loads the interrupt descriptor table base/limit;
        // the table is fully initialized above and static.
        x86_load_idt(&idtr);

        // ---- PIC 8259 (before any interrupt can fire; IF is still off) ----
        init_8259();
    }
}


/// Per-CPU trap bring-up on an AP: FPU, its own GDT/TSS (LTR), the
/// shared IDT, and the syscall MSRs (STAR/LSTAR/FMASK are per-CPU).
pub fn init_secondary(cpu: usize) {
    // SAFETY: one-time per-CPU feature setup, interrupts still off.
    unsafe {
        super::thread::fpu_init();
        init_cpu_state(cpu);
        let idtr = PseudoDescriptor {
            limit: (core::mem::size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: &raw mut IDT_X86 as usize as u64,
        };
        x86_load_idt(&idtr);
    }
    init_syscall();
}

/// Initialize syscall MSRs (STAR/LSTAR/FMASK) and EFER.SCE.
///
/// Also (re-)asserts EFER.NXE: user PTEs carry the NX bit (exec's
/// per-segment W^X tighten), and with NXE=0 bit 63 is reserved — every
/// access to a data page faults #PF e=0xC (RSVD).  The BSP gets NXE
/// from boot.S; APs come out of the trampoline with only LME set, and
/// this is the first EFER write on their kernel path.
pub fn init_syscall() {
    extern "C" {
        static syscall_entry: u8;
    }
    // SAFETY: MSR writes configure the syscall entry; values are the
    // constants documented at the top of this file.
    unsafe {
        let efer = rdmsr(MSR_EFER);
        wrmsr(MSR_EFER, efer | EFER_SCE | EFER_NXE);
        wrmsr(MSR_STAR, MSR_STAR_VALUE);
        wrmsr(MSR_LSTAR, &raw const syscall_entry as u64);
        wrmsr(MSR_FMASK, MSR_FMASK_VALUE);
    }
}

// ============================================================================
// PIC 8259
// ============================================================================

const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

/// PIC IRQ base vectors (master 0x20-0x27, slave 0x28-0x2f).
pub const PIC_IRQ_BASE: u8 = 0x20;

/// Legacy PC/AT IRQ line numbers.
pub const IRQ_TIMER: u8 = 0;
pub const IRQ_CASCADE: u8 = 2;

/// Standard Linux-style init_8259A: ICW1-4 remap to vectors 32..47,
/// cascade wiring, then mask everything except the cascade line.
fn init_8259() {
    // SAFETY: port writes program the 8259 during early init with IF off.
    unsafe {
        outb(PIC1_CMD, 0x11); // ICW1: init, edge-triggered, cascade, ICW4
        super::cpu::io_wait();
        outb(PIC2_CMD, 0x11);
        super::cpu::io_wait();
        outb(PIC1_DATA, PIC_IRQ_BASE); // ICW2: master vector base 0x20
        super::cpu::io_wait();
        outb(PIC2_DATA, PIC_IRQ_BASE + 8); // ICW2: slave vector base 0x28
        super::cpu::io_wait();
        outb(PIC1_DATA, 1 << IRQ_CASCADE); // ICW3: slave hangs off IRQ2
        super::cpu::io_wait();
        outb(PIC2_DATA, IRQ_CASCADE); // ICW3: slave identity 2
        super::cpu::io_wait();
        outb(PIC1_DATA, 0x01); // ICW4: 8086 mode
        super::cpu::io_wait();
        outb(PIC2_DATA, 0x01);
        super::cpu::io_wait();

        // OCW1: mask all lines except the cascade (bit 2 of the master).
        outb(PIC1_DATA, 0xFF & !(1 << IRQ_CASCADE));
        outb(PIC2_DATA, 0xFF);
    }
}

/// Non-specific EOI — conservative like early Linux: always EOI the
/// master, and the slave too when the line is on it.
fn pic_send_eoi(line: u8) {
    // SAFETY: EOI writes are idempotent at the chip.
    unsafe {
        if line >= 8 {
            outb(PIC2_CMD, 0x20);
        }
        outb(PIC1_CMD, 0x20);
    }
}

/// Claim the highest-priority in-service IRQ (PIC analogue of the PLIC
/// claim read): switch both chips to ISR-read mode and pick the lowest
/// set line (8259 priority order).  None means spurious.
fn pic_claim() -> Option<u8> {
    // SAFETY: OCW3 ISR-mode selects are read-side only.
    unsafe {
        outb(PIC1_CMD, 0x0B); // OCW3: read ISR on next read
        outb(PIC2_CMD, 0x0B);
        let isr = ((inb(PIC2_CMD) as u16) << 8) | inb(PIC1_CMD) as u16;
        if isr == 0 {
            return None;
        }
        Some(isr.trailing_zeros() as u8)
    }
}

/// Unmask one PIC line (0..15).
pub fn unmask_irq_line(line: u8) {
    // SAFETY: RMW of the mask register; IF-off or single-owner callers.
    unsafe {
        if line < 8 {
            let m = inb(PIC1_DATA) & !(1 << line);
            outb(PIC1_DATA, m);
        } else if line < 16 {
            let m = inb(PIC2_DATA) & !(1 << (line - 8));
            outb(PIC2_DATA, m);
        }
    }
}

/// Mask one PIC line (0..15).
pub fn mask_irq_line(line: u8) {
    // SAFETY: RMW of the mask register.
    unsafe {
        if line < 8 {
            let m = inb(PIC1_DATA) | (1 << line);
            outb(PIC1_DATA, m);
        } else if line < 16 {
            let m = inb(PIC2_DATA) | (1 << (line - 8));
            outb(PIC2_DATA, m);
        }
    }
}

// ============================================================================
// IRQ line dispatch table (trap.rs-owned; the generic irq-domain layer
// is PLIC-shaped and arrives with the LAPIC/IOAPIC phase)
// ============================================================================

type IrqLineHandler = fn();

static IRQ_LINE_HANDLERS: [core::sync::atomic::AtomicU64; 16] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 16];

/// Register a handler for a PIC line (0..15) and unmask it.  Returns
/// false when the line is taken or out of range.
pub fn request_irq_line(line: u8, handler: fn()) -> bool {
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
    unmask_irq_line(line);
    true
}

pub fn free_irq_line(line: u8) {
    use core::sync::atomic::Ordering;
    if line < 16 {
        mask_irq_line(line);
        IRQ_LINE_HANDLERS[line as usize].store(0, Ordering::Release);
    }
}

// ============================================================================
// Interrupt control (doc-comment contracts from the skeleton)
// ============================================================================

/// Enable the timer interrupt: arm the per-CPU LAPIC timer when the
/// APIC driver is up (SMP tick source), else the legacy PIT path
/// (unmask PIC IRQ0), then open the CPU gate (STI).
///
/// X86-CLK: the PIT IRQ0 line is only masked when the LAPIC timer
/// actually armed. Masking it unconditionally used to leave a failed
/// calibration (no LAPIC frequency) with NO tick source at all —
/// jiffies frozen, timer softirq never raised, nanosleep asleep
/// forever.
pub fn enable_timer_interrupt() {
    let mut lapic_armed = false;
    if crate::drivers::intc::apic::ready() {
        lapic_armed = crate::drivers::intc::apic::timer_start();
    }
    if lapic_armed {
        // The free-running PIT tick (if it was ever unmasked) would now
        // race the LAPIC tick into the jiffies grid; mask it off.
        mask_irq_line(IRQ_TIMER);
    } else {
        // PIT fallback tick: program channel 0 at HZ (nothing programs
        // it on the LAPIC path — QEMU's reset default is the 18.2 Hz
        // DOS divider) and open the line.
        crate::drivers::timer::init();
        crate::drivers::timer::set_next_trigger();
        unmask_irq_line(IRQ_TIMER);
    }
    // SAFETY: sti enables interrupts at the end of boot bring-up, the
    // x86 mirror of the twin's sstatus.SIE step.
    unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
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
static CURRENT_PT_REGS: [core::sync::atomic::AtomicU64; crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; crate::config::MAX_CPUS];

pub fn current_pt_regs() -> *const PtRegs {
    let cpu = crate::arch::cpu_id() as usize;
    CURRENT_PT_REGS[cpu].load(core::sync::atomic::Ordering::Relaxed) as *const PtRegs
}

/// pt_regs of the current task's user frame (stack_top - sizeof(PtRegs))
pub fn current_task_pt_regs() -> Option<&'static mut PtRegs> {
    let task = crate::sched::current()?;
    // SAFETY: kernel stack top is allocated by the task subsystem; the user
    // entry frame sits at the fixed offset from it.
    unsafe {
        let stack_top = (*task).get_kernel_stack()? as u64;
        Some(&mut *((stack_top - PT_REGS_SIZE as u64) as *mut PtRegs))
    }
}

// ============================================================================
// Trap dispatch
// ============================================================================

/// x86 vector numbers used in the match below.
mod vec {
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
    pub const CP: u64 = 21; // control protection
    pub const IRQ0: u64 = 32; // PIC timer vector (virtual-wire fallback)
    pub const IRQ_LAST: u64 = 47; // PIC slave line 15 vector
}

/// LAPIC timer vector (per-CPU tick; same handler body as the PIC path).
const LOCAL_TIMER: u64 = crate::drivers::intc::apic::LOCAL_TIMER_VECTOR as u64;

/// Trap handler — called by trap.S stubs with the frame pointer, CPU id,
/// and the raw vector number (third argument: x86 has no scause-like
/// frame field, so the stub passes its identity; the error code stays in
/// the orig_rax slot).
#[no_mangle]
pub extern "C" fn trap_handler(regs: *mut PtRegs, cpu_id: usize, vector: u64) {
    // RACE-FORENSICS (x86-smprace): the stub read the CPU number from
    // %gs:PC_CPU_NUMBER — an out-of-range value means the active GS base
    // is NOT the per-CPU slot (GS pairing broken): every %gs-relative
    // access below would hit arbitrary memory. Name it and halt instead
    // of corrupting.
    if cpu_id >= crate::config::MAX_CPUS {
        use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
        taskdump_raw_line(b"\nGS-HELL cpu_id=");
        taskdump_dec(cpu_id as u64);
        taskdump_raw_line(b" vector=0x");
        taskdump_dec(vector);
        taskdump_raw_line(b" rip=0x");
        taskdump_dec(unsafe { (*regs).rip });
        taskdump_raw_line(b" gsbase_msr=0x");
        let gsbase = unsafe { rdmsr(0xC000_0101) };
        taskdump_dec(gsbase);
        taskdump_raw_line(b"\n");
        // SAFETY: the kernel cannot proceed with a broken GS base.
        loop {
            unsafe { core::arch::asm!("cli; hlt") };
        }
    }
    // SAFETY: regs points to a valid PtRegs built by a trap.S stub; the
    // pointer stays valid for the duration of this handler.
    unsafe {
        // Save and swap PtRegs pointer (used for fork/exec).  Nesting
        // save/restore mirrors the riscv64 twin: a timer IRQ landing
        // inside a syscall must not clobber the outer syscall's pointer.
        let prev_pt_regs = CURRENT_PT_REGS[cpu_id].load(core::sync::atomic::Ordering::Relaxed);
        CURRENT_PT_REGS[cpu_id].store(regs as u64, core::sync::atomic::Ordering::Relaxed);

        let regs_ref = &mut *regs;

        crate::pr_debug!(
            "trap: vector={:#x}, rip={:#x}, rsp={:#x}, mode={}",
            vector,
            regs_ref.rip,
            regs_ref.rsp,
            if regs_ref.user_mode() { "user" } else { "kernel" }
        );

        if vector == SYSCALL_VECTOR {
            handle_syscall(regs_ref);
        } else {
            match vector {
                vec::PF => handle_page_fault(regs_ref),

                vec::UD => handle_illegal_instruction(regs_ref),
                vec::GP => handle_general_protection(regs_ref),

                vec::DE | vec::OF | vec::BR | vec::MF => handle_arith_exception(regs_ref, vector),
                vec::AC => handle_alignment_check(regs_ref),

                vec::BP => handle_breakpoint(regs_ref),
                vec::DB => handle_debug(regs_ref),

                vec::NM => handle_device_not_available(),

                vec::NMI => {
                    // No NMI source is configured on this board (no PMU/
                    // perfctr wiring); a stray NMI is worth one line.
                    crate::pr_warn!("trap: unexpected NMI at rip={:#x}", regs_ref.rip);
                }

                LOCAL_TIMER => {
                    crate::interrupt::preempt::irq_enter();
                    // LAPIC EOI FIRST (same discipline as the PIC path):
                    // the scheduler call below can switch tasks, and the
                    // in-service bit would block further delivery.
                    crate::drivers::intc::apic::eoi();
                    handle_timer_tick(regs_ref, cpu_id);
                    crate::interrupt::preempt::irq_exit();
                }

                v if (crate::arch::ipi::IPI_FIRST..=crate::arch::ipi::IPI_LAST)
                    .contains(&v) =>
                {
                    crate::interrupt::preempt::irq_enter();
                    crate::drivers::intc::apic::eoi();
                    crate::arch::ipi::handle_ipi_vector(v);
                    crate::interrupt::preempt::irq_exit();
                }

                vec::IRQ0 => {
                    crate::interrupt::preempt::irq_enter();
                    handle_timer_interrupt(regs_ref, cpu_id);
                    crate::interrupt::preempt::irq_exit();
                }

                v if v > vec::IRQ0 && v <= vec::IRQ_LAST => {
                    crate::interrupt::preempt::irq_enter();
                    handle_external_interrupt(v);
                    crate::interrupt::preempt::irq_exit();
                }

                _ => handle_unknown_exception(regs_ref, vector),
            }
        }

        // Restore previous PtRegs pointer (handles nested interrupt case)
        CURRENT_PT_REGS[cpu_id].store(prev_pt_regs, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Shared timer-tick body (used by both the LAPIC-timer vector and the
/// legacy PIC IRQ0 fallback) — faithful port of the riscv64 twin.
fn handle_timer_tick(regs: &mut PtRegs, cpu: usize) {
    // Increment interrupt counter for /proc/interrupts
    crate::fs::procfs::interrupts::timer_inc(cpu);

    // Skip scheduler logic during early boot (no current task).
    if crate::sched::current().is_none() {
        return;
    }

    // 0. CPU-time accounting: charge the tick to the interrupted task,
    // user or system time by the mode the timer hit (user tasks only).
    if let Some(task) = crate::sched::current() {
        // SAFETY: current() returned a live task pointer; we only touch
        // per-task atomic counters, no teardown race on these fields.
        unsafe {
            if (*task).address_space().is_some() {
                use core::sync::atomic::Ordering::Relaxed;
                if regs.user_mode() {
                    (*task).utime_ticks.fetch_add(1, Relaxed);
                } else {
                    (*task).stime_ticks.fetch_add(1, Relaxed);
                }
            }
        }
    }

    // 1. Update jiffies
    crate::drivers::timer::timer_interrupt_handler();

    // 1.5 Scribble hunter (dfx=scribble): GS-pairing tripwire + verify
    // quiescent tasks' tracked scheduling fields against their shadows +
    // scheduler-consistency scan (double-run seed detection).
    // Throttled inside; no-op unless the runtime switch is on.
    crate::dfx::scribble::verify_gs();
    crate::dfx::scribble::verify();
    crate::dfx::scribble::verify_consistency();

    // 2. Scheduler tick
    crate::sched::scheduler_tick();

    // 3. Check for soft lockups
    crate::dfx::softlockup::check();

    // 4. Reschedule if needed (the trap-exit asm path owns the user-mode
    //    preemption; this in-IRQ switch only fires when preemptible,
    //    which is never true inside irq_enter — kept for twin parity).
    if crate::sched::need_resched() && crate::interrupt::preempt::preemptible() {
        crate::sched::schedule();
    }
}

/// Handle timer interrupt on the legacy PIC IRQ0 vector (virtual-wire
/// fallback; the LAPIC timer vector routes to `handle_timer_tick`
/// directly with a LAPIC EOI).
fn handle_timer_interrupt(regs: &mut PtRegs, cpu: usize) {
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
fn handle_external_interrupt(vector: u64) {
    // The timer line never reaches here (vector 32 has its own gate and
    // EOI-first handler), so every claimed line is a device line.
    let mut claimed = 0;
    while let Some(line) = pic_claim() {
        let h = IRQ_LINE_HANDLERS[line as usize].load(core::sync::atomic::Ordering::Acquire);
        if h != 0 {
            // SAFETY: nonzero values were stored from fn pointers by
            // request_irq_line.
            let f: fn() = unsafe { core::mem::transmute(h) };
            f();
        } else {
            crate::pr_debug!("trap: unhandled IRQ line {} (vector {:#x})", line, vector);
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
fn handle_syscall(regs: &mut PtRegs) {
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
fn handle_illegal_instruction(regs: &mut PtRegs) {
    // x86_64 has no lazy-FPU first-use trap (the FPU is enabled at boot
    // via CR4.OSFXSR in fpu_init and never gated with CR0.TS), so there
    // is no decode-and-retry path — straight to signal delivery.
    crate::pr_debug!("trap: #UD at rip={:#x}, mode={}",
        regs.rip, if regs.user_mode() { "user" } else { "kernel" });

    if regs.user_mode() {
        crate::process::exit::do_exit(-(crate::signal::Signal::SIGILL as i32));
        // Do NOT advance rip — the task is ZOMBIE and will not resume
    } else {
        // A kernel-mode #UD is always a kernel bug; skipping it would
        // corrupt execution.  Die loudly (twin discipline).
        #[cfg(feature = "x86_64")]
        crate::dfx::scribble::crash_report(b"#UD", regs);
        panic!("trap: illegal instruction in kernel mode at rip={:#x}", regs.rip);
    }
}

/// Handle #GP: user mode → SIGSEGV (Linux semantics), kernel → bug.
fn handle_general_protection(regs: &mut PtRegs) {
    crate::pr_debug!("trap: #GP at rip={:#x}, mode={}",
        regs.rip, if regs.user_mode() { "user" } else { "kernel" });

    if regs.user_mode() {
        // Handler-first routing (same as the page-fault Segfault path):
        // debuggers/crash catchers must see the SIGSEGV; the default
        // disposition kills here.
        let pid = crate::process::current_pid();
        if sigsegv_has_handler() {
            let _ = crate::signal::send_signal(pid, crate::signal::Signal::SIGSEGV as i32);
            return;
        }
        let _ = crate::signal::send_signal(pid, crate::signal::Signal::SIGSEGV as i32);
        crate::process::exit::do_exit(-(crate::signal::Signal::SIGSEGV as i32));
    } else {
        #[cfg(feature = "x86_64")]
        crate::dfx::scribble::crash_report(b"#GP", regs);
        panic!(
            "trap: #GP in kernel mode at rip={:#x}, error={:#x}",
            regs.rip, regs.orig_rax
        );
    }
}

/// Arithmetic exceptions: #DE / #OF / #BR / #XM → SIGFPE.
fn handle_arith_exception(regs: &mut PtRegs, vector: u64) {
    crate::pr_debug!("trap: arith exception {:#x} at rip={:#x}", vector, regs.rip);
    if regs.user_mode() {
        crate::process::exit::do_exit(-(crate::signal::Signal::SIGFPE as i32));
    } else {
        panic!("trap: arith exception {:#x} in kernel mode at rip={:#x}", vector, regs.rip);
    }
}

/// #AC (alignment check) → SIGBUS in user mode.
fn handle_alignment_check(regs: &mut PtRegs) {
    if regs.user_mode() {
        let pid = crate::process::current_pid();
        let _ = crate::signal::send_signal(pid, crate::signal::Signal::SIGBUS as i32);
        if !sigbus_has_handler() {
            crate::process::exit::do_exit(-(crate::signal::Signal::SIGBUS as i32));
        }
    } else {
        panic!("trap: #AC in kernel mode at rip={:#x}", regs.rip);
    }
}

/// #NM — FPU unavailable.  Should not happen (fpu_init clears CR0.EM/TS),
/// but re-initialize defensively and resume instead of dying.
fn handle_device_not_available() {
    crate::pr_warn!("trap: #NM (FPU unavailable) — re-running fpu_init");
    // SAFETY: fpu_init only flips CR0/CR4 feature bits and zeroes the
    // live FPU state; the caller was not using it (#NM proves that).
    unsafe { super::thread::fpu_init() };
}

/// #DB — hardware single-step (TF) or debug-register trap.
fn handle_debug(regs: &mut PtRegs) {
    const TF: u64 = 1 << 8;
    if regs.user_mode() {
        // Stop stepping before delivering anything, or iretq would
        // re-trap on the first handler/user instruction.
        regs.rflags &= !TF;
        if let Some(task) = crate::sched::current() {
            if (*task).tracer_pid() != 0 {
                // SAFETY: task is the current task in its trap path.
                unsafe {
                    crate::process::ptrace::ptrace_stop(
                        task,
                        crate::signal::Signal::SIGTRAP as i32,
                        crate::signal::SigInfo::new(
                            crate::signal::Signal::SIGTRAP as i32,
                            crate::signal::si_code::SI_KERNEL,
                            (*task).pid(),
                            0,
                        ),
                    );
                }
                return;
            }
        }
        crate::process::exit::do_exit(-(crate::signal::Signal::SIGTRAP as i32));
    } else {
        // Kernel-mode #DB with TF set: clear TF in the saved frame and
        // resume (Linux do_debug discipline for kernel steps).
        regs.rflags &= !TF;
    }
}

/// int3 (#BP) — port of the twin's handle_breakpoint for the int3
/// displacement scheme: PTRACE_SINGLESTEP replaces the instruction at
/// the target with int3 (1 byte).  int3 is a TRAP, so rip in the frame
/// already points one byte PAST the displaced instruction; the restore
/// path rewinds rip by 1 so the restored instruction re-executes.
fn handle_breakpoint(regs: &mut PtRegs) {
    if regs.user_mode() {
        if let Some(task) = crate::sched::current() {
            let step_addr = (*task).single_step_addr();
            if (*task).single_step_active() && regs.rip == step_addr.wrapping_add(1) {
                let saved = (*task).single_step_saved_insn();
                let restored = crate::process::ptrace::write_target_word(task, step_addr, saved);
                (*task).clear_single_step();
                if !restored {
                    // Tracee's text is gone racing the step — kill it.
                    crate::process::exit::do_exit(
                        -(crate::signal::Signal::SIGSEGV as i32),
                    );
                }
                // Rewind past the int3 so the restored instruction
                // re-executes (x86 trap already advanced rip).
                regs.rip = step_addr;
                if (*task).tracer_pid() != 0 {
                    // SAFETY: task is the current task in its trap path.
                    unsafe {
                        crate::process::ptrace::ptrace_stop(
                            task,
                            crate::signal::Signal::SIGTRAP as i32,
                            crate::signal::SigInfo::new(
                                crate::signal::Signal::SIGTRAP as i32,
                                crate::signal::si_code::SI_KERNEL,
                                (*task).pid(),
                                0,
                            ),
                        );
                    }
                } else {
                    (*task).pending.add(crate::signal::Signal::SIGTRAP as i32);
                }
                return;
            }
            // A real user int3 under a tracer stops for the tracer
            // (gdb inserts and relies on breakpoints); rip already
            // points after the 1-byte int3, matching Linux do_int3.
            if (*task).tracer_pid() != 0 {
                // SAFETY: task is the current task in its trap path.
                unsafe {
                    crate::process::ptrace::ptrace_stop(
                        task,
                        crate::signal::Signal::SIGTRAP as i32,
                        crate::signal::SigInfo::new(
                            crate::signal::Signal::SIGTRAP as i32,
                            crate::signal::si_code::SI_KERNEL,
                            (*task).pid(),
                            0,
                        ),
                    );
                }
                return;
            }
        }
        crate::process::exit::do_exit(-(crate::signal::Signal::SIGTRAP as i32));
        // Do NOT advance rip — the task is ZOMBIE and will not resume
    } else {
        // Kernel-mode int3 is a kernel bug (kgdb-style breakpoints are
        // not wired up).
        panic!("trap: int3 in kernel mode at rip={:#x}", regs.rip);
    }
}

// ============================================================================
// Page fault
// ============================================================================

/// True when the CURRENT task has a user handler installed for SIGSEGV
/// (port of the twin's helper — decides pend-vs-kill routing).
fn sigsegv_has_handler() -> bool {
    crate::sched::current()
        .and_then(|t| (*t).signal.as_ref().map(|s| s.get_action(11)))
        .map(|a| a.is_some_and(|a| a.has_handler()))
        .unwrap_or(false)
}

fn sigbus_has_handler() -> bool {
    crate::sched::current()
        .and_then(|t| (*t).signal.as_ref().map(|s| s.get_action(7)))
        .map(|a| a.is_some_and(|a| a.has_handler()))
        .unwrap_or(false)
}

/// Handle page fault (#PF): decode the error code (in the orig_rax slot)
/// into FaultFlags and hand off to `exception::do_page_fault`, then
/// mirror the twin's outcome routing.
fn handle_page_fault(regs: &mut PtRegs) {
    use crate::arch::mm::exception::do_page_fault;
    use crate::arch::mm::page_fault::MmFaultResult;

    let error_code = regs.orig_rax;
    let fault_addr = super::cpu::read_cr2();

    // Error-code bits: 1 W (write), 2 U (user), 4 I/D (instruction fetch).
    let mut access = 0u32;
    if error_code & 0x2 != 0 {
        access |= crate::arch::mm::page_fault::FaultFlags::WRITE;
    } else {
        access |= crate::arch::mm::page_fault::FaultFlags::READ;
    }
    if error_code & 0x10 != 0 {
        access |= crate::arch::mm::page_fault::FaultFlags::EXEC;
    }
    if error_code & 0x4 != 0 {
        access |= crate::arch::mm::page_fault::FaultFlags::USER;
    }
    // Reserved-bit walk fault (ec bit 3): a paging-structure entry carries
    // a bit the current CR4/EFER setup reserves (classically NX with
    // EFER.NXE=0). Never a legitimate user condition — fail loudly
    // instead of letting it decode as a plain read fault and loop.
    if error_code & 0x8 != 0 {
        panic!(
            "#PF RSVD-bit set in paging structures: addr={:#x} rip={:#x} ec={:#x} (EFER.NXE off with NX PTEs?)",
            fault_addr, regs.rip, error_code
        );
    }

    crate::pr_debug!(
        "trap: page fault addr={:#x}, rip={:#x}, ec={:#x}, mode={}",
        fault_addr,
        regs.rip,
        error_code,
        if regs.kernel_mode() { "kernel" } else { "user" }
    );

    let result = do_page_fault(regs, access);

    match result {
        MmFaultResult::Handled | MmFaultResult::Fixed => {
            // Page handled, re-execute instruction
        }
        MmFaultResult::Segfault => {
            crate::pr_err!(
                "pagefault: Segfault at {:#x}, rip={:#x}, rsp={:#x}, pid={}, mode={}",
                fault_addr,
                regs.rip,
                regs.rsp,
                crate::sched::get_current_pid(),
                if regs.kernel_mode() { "kernel" } else { "user" }
            );
            if regs.user_mode() {
                // Linux semantics: route user-mode faults through signal
                // delivery (force_sig_fault) — an installed SIGSEGV
                // handler must run (LTP mmap05); default kills here.
                let pid = crate::process::current_pid();
                if sigsegv_has_handler() {
                    let _ = crate::signal::send_signal(pid, crate::signal::Signal::SIGSEGV as i32);
                    return;
                }
                let _ = crate::signal::send_signal(pid, crate::signal::Signal::SIGSEGV as i32);
                crate::process::exit::do_exit(-(crate::signal::Signal::SIGSEGV as i32));
            } else {
                #[cfg(feature = "x86_64")]
                crate::dfx::scribble::crash_report(b"#PF-segv", regs);
                // The riscv64 mm layer returns KernelPanic for kernel
                // faults; until the x86 twin grows that, do NOT silently
                // iretq back into the faulting rip (infinite fault loop).
                panic!(
                    "pagefault: Segfault in kernel mode at {:#x}, rip={:#x}",
                    fault_addr, regs.rip
                );
            }
        }
        MmFaultResult::PermissionDenied => {
            crate::pr_err!("pagefault: Permission denied at {:#x}, rip={:#x}, pid={}",
                fault_addr, regs.rip, crate::sched::get_current_pid());
            if regs.user_mode() {
                // Handler-first routing like Segfault (PROT_NONE access).
                let pid = crate::process::current_pid();
                if sigsegv_has_handler() {
                    let _ = crate::signal::send_signal(pid, crate::signal::Signal::SIGSEGV as i32);
                    return;
                }
                crate::process::exit::do_exit(-(crate::signal::Signal::SIGSEGV as i32));
            } else {
                #[cfg(feature = "x86_64")]
                crate::dfx::scribble::crash_report(b"#PF-perm", regs);
                panic!(
                    "pagefault: PermissionDenied in kernel mode at {:#x}, rip={:#x}",
                    fault_addr, regs.rip
                );
            }
        }
        MmFaultResult::BusError => {
            crate::pr_err!("pagefault: Bus error (past EOF) at {:#x}", fault_addr);
            if regs.user_mode() {
                let pid = crate::process::current_pid();
                let _ = crate::signal::send_signal(pid, crate::signal::Signal::SIGBUS as i32);
                if !sigbus_has_handler() {
                    crate::process::exit::do_exit(-(crate::signal::Signal::SIGBUS as i32));
                }
            } else {
                #[cfg(feature = "x86_64")]
                crate::dfx::scribble::crash_report(b"#PF-bus", regs);
                panic!(
                    "pagefault: BusError in kernel mode at {:#x}, rip={:#x}",
                    fault_addr, regs.rip
                );
            }
        }
        MmFaultResult::OutOfMemory => {
            crate::pr_err!("pagefault: Out of memory at {:#x}", fault_addr);
            crate::mm::page_alloc::oom_forensic_dump("pagefault OOM");
            if regs.user_mode() {
                crate::process::exit::do_exit(-(crate::signal::Signal::SIGKILL as i32));
            } else {
                panic!(
                    "pagefault: OOM in kernel mode at {:#x}, rip={:#x}",
                    fault_addr, regs.rip
                );
            }
        }
        MmFaultResult::KernelPanic => {
            // Raw serial print (printk may be wedged on a lock this CPU
            // holds), then halt — the twin's R9 discipline.
            // putchar_no_lock writes the serial port directly (no locks).
            // (x86-smprace: the old (0..64).step_by(4).rev() loops hit a
            // shift-overflow panic on their final iterator step; the
            // explicit while form below cannot.)
            fn kp_put_hex(v: u64) {
                let mut sh: i32 = 64;
                while sh > 0 {
                    sh -= 4;
                    let nb = ((v >> sh) & 0xF) as u8;
                    crate::console::putchar_no_lock(if nb < 10 {
                        b'0' + nb
                    } else {
                        b'a' + nb - 10
                    });
                }
            }
            for &b in b"trap: KERNPANIC pfault badaddr=0x" {
                crate::console::putchar_no_lock(b);
            }
            kp_put_hex(fault_addr);
            for &b in b" rip=0x" {
                crate::console::putchar_no_lock(b);
            }
            kp_put_hex(regs.rip);
            for &b in b" rsp=0x" {
                crate::console::putchar_no_lock(b);
            }
            kp_put_hex(regs.rsp);
            crate::console::putchar_no_lock(b'\n');
            // SAFETY: hlt halts until the next interrupt; safe in a halt loop.
            loop {
                unsafe { core::arch::asm!("hlt") };
            }
        }
        _ => {}
    }
}

/// Handle unknown exception (port of the twin's handler).
fn handle_unknown_exception(regs: &mut PtRegs, vector: u64) {
    let cause = Cause::from_cause(vector);
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
        // the instruction corrupts state.
        panic!(
            "trap: unknown exception {:#x} in kernel mode at rip={:#x}",
            vector, regs.rip
        );
    }
}

// ============================================================================
// asm-callable helpers
// ============================================================================

/// Preemption gate for the trap-exit asm path (the generic preemptible()
/// is not #[no_mangle] extern "C"; this thin wrapper is the x86 twin of
/// the riscv64 asm's direct ti_preempt_count read).
#[no_mangle]
pub extern "C" fn x86_preemptible() -> i64 {
    if crate::interrupt::preempt::preemptible() {
        1
    } else {
        0
    }
}

/// Double-fault dump (IST1 stub target).  Raw serial only: no locks, no
/// allocation, no printk machinery — the kernel state is unrecoverable.
#[no_mangle]
pub extern "C" fn x86_double_fault_dump() {
    use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
    let rsp: u64;
    // SAFETY: reading rsp is side-effect free.
    unsafe { core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack)) };
    taskdump_raw_line(b"\ntrap: DOUBLE FAULT (#8) on IST1, rsp=");
    taskdump_dec(rsp);
    taskdump_raw_line(b" - kernel state unrecoverable, halting\n");
}

/// ret_from_fork_kernel target: run the kthread function and exit.
/// Never returns (mirror of the riscv64 twin's ret_from_fork_kernel).
#[no_mangle]
pub extern "C" fn ret_from_fork_kernel_helper(
    fn_arg: *mut core::ffi::c_void,
    fn_ptr: extern "C" fn(*mut core::ffi::c_void) -> i32,
    _regs: *mut PtRegs,
) {
    let ret = fn_ptr(fn_arg);
    crate::process::exit::do_exit(ret);
}
