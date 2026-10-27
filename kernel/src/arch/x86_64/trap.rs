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
#[no_mangle]
#[unsafe(naked)]
pub unsafe extern "C" fn ret_from_fork() {
    // SAFETY: naked trampoline; contract documented in the trap.S comment
    // block it replaced (fork sets thread.sp = frame, callee.ret_addr = here).
    unsafe {
        core::arch::naked_asm!(
            "movq %rsp, %rbp",
            "andq $-16, %rsp",
            "call get_prev_task",
            "movq %rax, %rdi",
            "call schedule_tail",
            "movq %rbp, %rsp",
            "jmp trap_exit",
            options(att_syntax)
        );
    }
}

#[no_mangle]
#[unsafe(naked)]
pub unsafe extern "C" fn ret_from_fork_kernel() {
    // SAFETY: naked trampoline; never returns — the helper calls do_exit.
    // R12 = kthread fn, R13 = fn_arg (switch-restored), RSP = scratch regs.
    unsafe {
        core::arch::naked_asm!(
            "movq %rsp, %rbp",
            "andq $-16, %rsp",
            "call get_prev_task",
            "movq %rax, %rdi",
            "call schedule_tail",
            "movq %r13, %rdi",
            "movq %r12, %rsi",
            "movq %rbp, %rdx",
            "call ret_from_fork_kernel_helper",
            "1:  cli",
            "    hlt",
            "    jmp 1b",
            options(att_syntax)
        );
    }
}

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

/// Wakewatch: RIP of another CPU's most recent trap frame, for the
/// TIMER-SOFTIRQ-STALL report. Diagnostic read of a slot the target CPU
/// owns; races live execution (worst case a torn-but-bounded value).
/// 0 when no frame is live or the slot holds a non-canonical pointer.
pub fn pt_regs_rip_for_cpu(cpu: usize) -> u64 {
    if cpu >= crate::config::MAX_CPUS {
        return 0;
    }
    let p = CURRENT_PT_REGS[cpu].load(core::sync::atomic::Ordering::Relaxed) as *const PtRegs;
    // High-half bound added with the pa15 lesson: user addresses are
    // canonical too, and this slot can only hold kernel-stack frames —
    // a value below the kernel-half base is a torn read, never deref it.
    if p.is_null()
        || (p as u64) < 0xffff_8000_0000_0000
        || !crate::sched::sched::kernel_ptr_canonical(p as *const u8)
    {
        return 0;
    }
    // SAFETY: canonical kernel address; the frame page stays mapped
    // (physmap/linear addresses of kernel stacks are never unmapped),
    // so the scalar read cannot fault — any value is diagnostic-only.
    unsafe { core::ptr::addr_of!((*p).rip).read() }
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

/// R67 (scribbled-frame guard): trap_exit's exit validation rejected the
/// frame `regs` — a kernel-origin tail whose rip is outside the kernel
/// image, or a user tail without the exact user selectors. That is the
/// residual x86 wild-jump family's loading point: a parked frame whose
/// control words were overwritten by a wild writer (observed shapes:
/// rip=0x4c/cs=8/rflags=0xc13, rip=user-code/cs=8, torn
/// rip=0x30ff03f8_80434c81, rip=stack-address) used to be fed straight
/// to iretq, executing user page-0 bytes in kernel mode and cascading
/// into the panic-recursion captures (b18/w75: identical frames, rsi=0x40,
/// rflags TF|DF set, IF clear). Linux's answer for this class is
/// fixup_bad_iret: the frame's owner dies, the system lives. Dump
/// forensics first (the moment of detection is the closest we ever get
/// to the writer), then do_exit — this never returns.
#[no_mangle]
pub extern "C" fn x86_bad_frame_exit(regs: *mut PtRegs) -> ! {
    // Forensics: full scribble crash capture (raw console; gated on
    // dfx=scribble inside crash_report itself).
    if !regs.is_null() {
        // SAFETY: diagnostic reads of the frame trap_exit just validated
        // (and rejected); the memory is the current task's stack.
        unsafe {
            crate::dfx::scribble::crash_report(b"BAD-FRAME", &*regs);
        }
    }
    // Route the owner out like any fatal-user-fault: SIGSEGV, default
    // disposition kills. do_exit never returns.
    crate::process::exit::do_exit(-(crate::signal::Signal::SIGSEGV as i32));
    unreachable!("do_exit returned from bad-frame exit");
}

/// Is `frame` inside `task`'s kernel stack [bottom, top)?  False for null
/// tasks and unowned early-boot entries (caller checks null separately).
fn frame_in_task_stack(task: *mut crate::process::task::Task, frame: usize) -> bool {
    // SAFETY: header reads of a task the per-CPU slot names as current.
    unsafe {
        let top = (*task).get_kernel_stack().map_or(0, |p| p as usize);
        if top == 0 {
            // No owned stack recorded (early boot / idle special paths) —
            // cannot validate; allow rather than false-positive.
            return true;
        }
        let bottom = (*task).kernel_stack_bottom();
        frame >= bottom && frame < top
    }
}

fn scribble_put_hex(mut v: u64) {
    for _ in 0..16 {
        let n = (v >> 60) as u8;
        crate::console::putchar_no_lock(if n < 10 { b'0' + n } else { b'a' + n - 10 });
        v <<= 4;
    }
}

fn scribble_put_str(s: &[u8]) {
    for &b in s {
        crate::console::putchar_no_lock(b);
    }
}

/// Raw (lock-free) report of a user-origin trap frame landing outside the
/// current task's kernel stack — the cross-stack scribble engine, named at
/// the moment of entry with the full per-CPU ownership picture.
fn scribble_foreign_entry(
    cpu: usize,
    vector: u64,
    regs: *mut PtRegs,
    tp: *mut crate::process::task::Task,
    frame: usize,
) {
    scribble_put_str(b"\n=== FOREIGN-ENTRY cpu=");
    scribble_put_hex(cpu as u64);
    scribble_put_str(b" vector=0x");
    scribble_put_hex(vector);
    scribble_put_str(b" frame=0x");
    scribble_put_hex(frame as u64);
    // SAFETY: diagnostic header reads of the task named by the slot.
    unsafe {
        scribble_put_str(b" task=0x");
        scribble_put_hex(tp as u64);
        scribble_put_str(b" pid=");
        scribble_put_hex((*tp).pid() as u64);
        scribble_put_str(b" kstack=[0x");
        scribble_put_hex((*tp).kernel_stack_bottom() as u64);
        scribble_put_str(b",0x");
        scribble_put_hex((*tp).get_kernel_stack().map_or(0, |p| p as u64));
        scribble_put_str(b") rsp0_mirror=0x");
        scribble_put_hex(super::smp::PER_CPU[cpu].tss_rsp0.load(core::sync::atomic::Ordering::Relaxed));
        scribble_put_str(b" TSS.rsp0=0x");
        let tss = &raw mut TSS_PER_CPU;
        scribble_put_hex((*tss)[cpu].read_u64(TSS_OFF_RSP0));
        scribble_put_str(b"\nFRAME:");
        for i in 0..21usize {
            if i % 4 == 0 {
                scribble_put_str(b"\n  ");
            }
            scribble_put_str(b" ");
            scribble_put_hex(core::ptr::read_volatile((frame + i * 8) as *const u64));
        }
        // Frame surroundings: the slots just above the frame belong to the
        // stack owner's earlier execution — torn words there name what the
        // two interleaved contexts were doing.
        scribble_put_str(b"\nABOVE:");
        let mut a = frame + 0xa8;
        while a < frame + 0xa8 + 0x100 {
            scribble_put_str(b" ");
            scribble_put_hex(core::ptr::read_volatile(a as *const u64));
            a += 8;
        }
    }
    scribble_put_str(b"\nSLOTS:");
    for c in 0..crate::config::MAX_CPUS {
        let t = super::smp::PER_CPU[c].current_task.load(core::sync::atomic::Ordering::Relaxed);
        scribble_put_str(b" c");
        let mut cc = c as u64;
        if cc >= 10 {
            crate::console::putchar_no_lock(b'0' + (cc / 10) as u8);
            cc %= 10;
        }
        crate::console::putchar_no_lock(b'0' + cc as u8);
        scribble_put_str(b"=0x");
        scribble_put_hex(t);
    }
    scribble_put_str(b"\n");
    crate::dfx::scribble::ring_dump();
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
    pub const XM: u64 = 19; // SIMD (SSE) math fault
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
    // SCRIBBLE2-GUARD (foreign-frame entry): a user-origin entry (trap,
    // IRQ, or syscall) must land inside the CURRENT task's kernel stack —
    // TSS.rsp0 / PER_CPU.tss_rsp0 name exactly that stack.  A frame
    // anywhere else is the cross-stack engine caught in the act: this CPU
    // would push frames through the victim's live data (torn return
    // addresses — the byte-flip/#GP/#UD family).  Report the full
    // ownership picture on raw serial and park THIS CPU (the others keep
    // their state for their own reports).
    if cpu_id < crate::config::MAX_CPUS {
        let user_entry = unsafe { ((*regs).cs & 3) != 0 || vector == SYSCALL_VECTOR };
        if user_entry {
            let tp = super::smp::PER_CPU[cpu_id]
                .current_task
                .load(core::sync::atomic::Ordering::Relaxed) as *mut crate::process::task::Task;
            let frame = regs as usize;
            if !tp.is_null() && !frame_in_task_stack(tp, frame) {
                scribble_foreign_entry(cpu_id, vector, regs, tp, frame);
                // SAFETY: executing further on a foreign stack only tears
                // the victim more; park this CPU with interrupts off.
                loop {
                    unsafe { core::arch::asm!("cli; hlt") };
                }
            }
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

        // OH bring-up: kernel-mode IPI/timer entries (0xE0-0xE5) are
        // routine events — printing every one flooded the emulated UART
        // (85% of a 400MB serial log) and slowed the whole boot enough
        // to cost samgr the hdf_devmgr race. The remaining per-trap
        // line (syscall entries are the overwhelming majority) is
        // likewise UART-flooding: it now needs dfx=steptrace.
        let routine_ipi = !regs_ref.user_mode()
            && (0xE0..=0xE5).contains(&vector);
        if !routine_ipi
            && crate::dfx::switches::enabled(
                crate::dfx::switches::DfxSwitch::Steptrace)
        {
            crate::pr_debug!(
                "trap: vector={:#x}, rip={:#x}, rsp={:#x}, mode={}",
                vector,
                regs_ref.rip,
                regs_ref.rsp,
                if regs_ref.user_mode() { "user" } else { "kernel" }
            );
        }

        if vector == SYSCALL_VECTOR {
            handle_syscall(regs_ref);
        } else {
            match vector {
                vec::PF => handle_page_fault(regs_ref),

                vec::UD => handle_illegal_instruction(regs_ref),
                vec::GP => handle_general_protection(regs_ref),

                vec::DE | vec::OF | vec::BR | vec::MF | vec::XM => {
                    handle_arith_exception(regs_ref, vector)
                }
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
                    // Fleet s2 session-6 discriminator, x86_64 twin: the
                    // interrupted task's OWN preempt_count at tick entry,
                    // before irq_enter books the tick's HARDIRQ_OFFSET.
                    // Lets a tick-side PREEMPT-LEAK pc=0x10001 split into
                    // "task already carried the bit" (pctick!=0) vs "the
                    // bit landed during the tick" (pctick==0). The riscv64
                    // twin has written this since fleet s2; the x86 side
                    // (the OH lane's arch) read a dead 0 until now.
                    crate::interrupt::preempt::TICK_ENTRY_PC[cpu_id].store(
                        crate::sched::current()
                            .map(|t| t.preempt_count())
                            .unwrap_or(0) as u32,
                        core::sync::atomic::Ordering::Relaxed,
                    );
                    // leg-33B: the interrupted RIP rides the same slot —
                    // the strand census's incumbent attribution
                    // (TICK_ENTRY_RIP).
                    crate::interrupt::preempt::TICK_ENTRY_RIP[cpu_id].store(
                        regs_ref.rip,
                        core::sync::atomic::Ordering::Relaxed,
                    );
                    crate::interrupt::preempt::irq_enter();
                    // LAPIC EOI FIRST (same discipline as the PIC path):
                    // the scheduler call below can switch tasks, and the
                    // in-service bit would block further delivery.
                    crate::drivers::intc::apic::eoi();
                    handle_timer_tick(regs_ref, cpu_id);
                    crate::interrupt::preempt::irq_exit();
                    irq_exit_maybe_preempt_kernel(regs_ref);
                }

                v if (crate::arch::ipi::IPI_FIRST..=crate::arch::ipi::IPI_LAST)
                    .contains(&v) =>
                {
                    crate::interrupt::preempt::irq_enter();
                    crate::drivers::intc::apic::eoi();
                    crate::arch::ipi::handle_ipi_vector(v);
                    crate::interrupt::preempt::irq_exit();
                    irq_exit_maybe_preempt_kernel(regs_ref);
                }

                vec::IRQ0 => {
                    crate::interrupt::preempt::irq_enter();
                    handle_timer_interrupt(regs_ref, cpu_id);
                    crate::interrupt::preempt::irq_exit();
                    irq_exit_maybe_preempt_kernel(regs_ref);
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

    // Leak-origin tripwire (dfx=leakhunt): this frame returns to
    // userspace, so the task's preempt_count must be exactly 0 (no
    // in-flight IRQ, every syscall-path guard dropped). A nonzero count
    // here is a leaked PREEMPT_OFFSET caught at the earliest possible
    // moment; the probe's per-task acquire-site shadow names the guard
    // construction that never balanced. Disabled-path cost: one relaxed
    // load. (Coroutine note: after an in-handler schedule() this tail
    // runs in the incoming task's own frozen frame with its own regs, so
    // the check validates whichever task actually returns to user.)
    if unsafe { (*regs).user_mode() } {
        crate::dfx::leakorigin::check_user_return();
    }
}

/// Rate gate for the IRQ-OFF-WINDOW probe: at most one direct-console
/// report per 2 s of jiffies (episodes can cluster; the log must not).
/// sc167: the 20-jiffy (200 ms) gate let sc166 print 2300 windows, and
/// every report is itself a ~130-char direct-console line whose THRE
/// waits run with IF=0 — the probe was feeding the very storm it
/// measured. 200 jiffies restores the documented 2 s cadence.
fn ww_irqwin_budget_ok() -> bool {
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
///   this hook (the trap-exit asm loop, cond_resched and lock_fair all
///   call schedule() sequentially, never from inside another one). A
///   leaked flag, should one ever happen, only disables preemption for
///   that task; count semantics and the sleep discipline stay exact.
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
fn irq_exit_maybe_preempt_kernel(regs: &PtRegs) {
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
    // stay as the safety net either way. Flip to true only after the
    // producer is fixed.
    const PREEMPT_HOOK_ENABLED: bool = false;
    if !PREEMPT_HOOK_ENABLED {
        return;
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
        crate::pr_info!(
            "sched: kernel-preempt enabled (first IRQ-exit switch, cpu={})",
            crate::arch::cpu_id()
        );
    }
    crate::sched::schedule();
}

/// Shared timer-tick body (used by both the LAPIC-timer vector and the
/// legacy PIC IRQ0 fallback) — faithful port of the riscv64 twin.
fn handle_timer_tick(regs: &mut PtRegs, cpu: usize) {
    // Increment interrupt counter for /proc/interrupts
    crate::fs::procfs::interrupts::timer_inc(cpu);

    // Wakewatch long-IRQ-off-window probe (UART perf gate round 2,
    // 2026-10-20): THIS tick arriving >100 ms after the previous one on
    // this CPU is the recovery IRQ of a long IF=0 window — the frame and
    // stack below are the culprit's, live, at the exact moment the window
    // broke (the pending tick fired at the first sti). Direct-console
    // print (IRQ context, R34 discipline), rate-gated to one per 2 s.
    // Symbolize the text-range words offline with addr2line.
    {
        let age_ms = crate::timer::ww_tick_age_ms(cpu);
        if age_ms > 100 && ww_irqwin_budget_ok() {
            use crate::console::putchar_no_lock as putchar;
            fn ph(v: u64) {
                use crate::console::putchar_no_lock as putchar;
                for sh in (0..16).rev() {
                    let n = ((v >> (sh * 4)) & 0xF) as u8;
                    putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                }
            }
            let cur = crate::sched::current();
            let cur_ptr = match cur {
                Some(t) => t as *mut crate::process::task::Task as usize,
                None => 0,
            };
            let (pid, pc) = unsafe {
                if cur_ptr != 0 {
                    let t = cur_ptr as *mut crate::process::task::Task;
                    ((*t).pid() as u64, (*t).preempt_count() as u64)
                } else {
                    (0, 0)
                }
            };
            // pa16 calibration (perf line, 2026-10-24): age is a WALL-CLOCK
            // gap between timer-IRQ arrivals, which conflates three causes
            // the in-guest clock cannot separate: a real IF=0 window, host
            // vCPU steal (the CPU simply not scheduled — dominant under
            // multi-QEMU host contention), and tickless-style idle (no
            // timer due, nothing to deliver). Their EFLAGS-sampling probe
            // measured max closed-cli 4 ms while this probe printed
            // age=1000ms lines — steal/idle, not IRQ-off. Annotation (not
            // silence): when the recovery tick lands on the idle task the
            // gap was idle/steal by construction, so say so and skip the
            // culprit walk; a real-task frame keeps the walk as a triage
            // hint only — corroborate with an EFLAGS sampler before
            // believing IF=0.
            let is_idle =
                cur_ptr != 0 && cur_ptr == crate::sched::sched::idle_task_ptr(cpu);
            let msg: &[u8] = b"\nWW: IRQ-OFF-WINDOW cpu=";
            for &b in msg {
                putchar(b);
            }
            ph(cpu as u64);
            let msg: &[u8] = b" age=";
            for &b in msg {
                putchar(b);
            }
            ph(age_ms);
            let msg: &[u8] = b"ms pid=";
            for &b in msg {
                putchar(b);
            }
            ph(pid);
            let msg: &[u8] = b" pc=";
            for &b in msg {
                putchar(b);
            }
            ph(pc);
            let msg: &[u8] = b" rip=";
            for &b in msg {
                putchar(b);
            }
            ph(regs.rip);
            let msg: &[u8] = b" rsp=";
            for &b in msg {
                putchar(b);
            }
            ph(regs.rsp);
            // Stack words in kernel text — return addresses of the
            // in-window call chain. Guarded by kernel-mode + kernel-half
            // + canonical + text-range checks; anything else prints '.'.
            // pa15 incident (perf line, 2026-10-24): the window can break
            // while the CPU is already executing USER code (recovery tick
            // interrupts a user frame), so regs.rsp is a USER stack
            // address — and user addresses are CANONICAL on x86_64, so
            // kernel_ptr_canonical alone let this walk read the user
            // stack and fault on its first unmapped page (KERNPANIC
            // pfault badaddr=user-page, rip=handle_timer_tick). A user
            // frame carries no kernel return addresses; skip the walk.
            // pa16: an idle recovery frame likewise implicates nobody —
            // the gap was idle/steal, not a culprit call chain.
            let (lo, hi) = crate::sched::sched::kernel_text_window();
            if !regs.user_mode()
                && !is_idle
                && regs.rsp >= 0xffff_8000_0000_0000
                && crate::sched::sched::kernel_ptr_canonical(regs.rsp as *const u8)
            {
                for i in 0..12 {
                    let addr = regs.rsp as *const u64;
                    // SAFETY: kernel-half canonical address; the kernel
                    // stack stays mapped, so the scalar read cannot fault.
                    let w = unsafe { core::ptr::addr_of!((*addr.add(i))).read_volatile() };
                    putchar(b' ');
                    if w >= lo && w < hi {
                        ph(w);
                    } else {
                        putchar(b'.');
                    }
                }
            } else if regs.user_mode() {
                let msg: &[u8] = b" user-frame";
                for &b in msg {
                    putchar(b);
                }
            } else if is_idle {
                let msg: &[u8] = b" idle-wake";
                for &b in msg {
                    putchar(b);
                }
            }
            putchar(b'\n');
        }
    }

    // Skip scheduler logic during early boot (no current task).
    if crate::sched::current().is_none() {
        return;
    }

    // 0. Per-CPU time accounting for /proc/stat (Linux
    // account_process_tick): the idle task means the CPU had nothing to
    // run, otherwise the tick is user time (the timer hit the task in
    // user mode) or system time (it hit it in kernel mode).
    crate::fs::procfs::stat::charge_tick(regs.user_mode());

    // 0b. CPU-time accounting: charge the tick to the interrupted task,
    // user or system time by the mode the timer hit (user tasks only).
    if let Some(task) = crate::sched::current() {
        // SAFETY: current() returned a live task pointer; we only touch
        // per-task atomic counters, no teardown race on these fields.
        unsafe {
            if (*task).address_space().is_some() {
                use core::sync::atomic::Ordering::Relaxed;
                if regs.user_mode() {
                    (*task).utime_ticks.fetch_add(1, Relaxed);
                    crate::dfx::syscost::tick_user();
                    // perf events (PERF_TYPE_SOFTWARE sampling): stash the
                    // interrupted PC for the softirq charger. Lock-free.
                    crate::perf::note_tick(regs.rip, crate::perf::MODE_USER, (*task).pid());
                } else {
                    (*task).stime_ticks.fetch_add(1, Relaxed);
                    // sc105 stime-burn attribution: sample WHERE the
                    // kernel-mode tick landed (4 KiB page bucket keyed by
                    // task comm, so one task's burn is separable from the
                    // boot crowd). KILLDUMP prints the table.
                    stime_sample_rip(regs.rip, (*task).comm());
                    crate::dfx::syscost::tick_kern(regs.rip);
                    crate::perf::note_tick(regs.rip, crate::perf::MODE_KERNEL, (*task).pid());
                }
            } else {
                crate::dfx::syscost::tick_idle();
                crate::perf::note_tick(regs.rip, crate::perf::MODE_IDLE, 0);
            }
        }
    } else {
        crate::dfx::syscost::tick_idle();
        crate::perf::note_tick(regs.rip, crate::perf::MODE_IDLE, 0);
    }

    // 1. Update jiffies
    crate::drivers::timer::timer_interrupt_handler();

    // 1.2 sc109: 1 Hz drift discipline for the HPET-calibrated TSC
    // timekeeper (offset-only forward steps, clamped; see
    // x86_64::resync_tsc_against_hpet).
    {
        let j = crate::drivers::timer::x86_64::get_jiffies();
        if j % crate::config::KERNEL_HZ as u64 == 0 {
            crate::drivers::timer::x86_64::resync_tsc_against_hpet();
        }
    }

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
    //
    // b143 (hdcd SIGILL loop, 15+ respawns): Linux routes user #UD
    // through force_sig_fault(SIGILL) — a process WITH a SIGILL handler
    // (OH's processdump/CFI crash reporter) gets to run it. Exiting
    // directly here killed hdcd before its handler could log the fault,
    // on every respawn. Same handler-first shape as the #GP path below.
    if regs.user_mode() {
        let pid = crate::process::current_pid();
        // Forensics before anything can unwind: rip + the first 16
        // instruction bytes (ud2 = 0f 0b is the compiler CFI/trap
        // fingerprint; anything else is a wild jump into data).
        {
            let comm = crate::sched::current()
                .map(|t| {
                    let c = unsafe { (*t).comm() };
                    let n = c.iter().position(|&b| b == 0).unwrap_or(c.len());
                    alloc::string::String::from(
                        core::str::from_utf8(&c[..n]).unwrap_or("?"),
                    )
                })
                .unwrap_or_default();
            let mut hex = alloc::string::String::new();
            if let Some(aspace) =
                crate::sched::current().and_then(|t| t.address_space())
            {
                let root = aspace.root_ppn();
                const HEX: &[u8; 16] = b"0123456789abcdef";
                for i in 0..16usize {
                    let va = regs.rip + i as u64;
                    let byte = (|| {
                        let phys = unsafe {
                            crate::arch::mm::mm_ops::PageTableWalker::walk(
                                root,
                                va & !0xfff,
                            )?
                        };
                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                            crate::arch::mm::memory_layout::PhysAddr::new(
                                (phys.0 << 12) | (va & 0xfff),
                            ),
                        );
                        // SAFETY: mapped user page through the linear map.
                        Some(unsafe {
                            core::ptr::read_volatile(kv.as_u64() as *const u8)
                        })
                    })();
                    match byte {
                        Some(b) => {
                            hex.push(HEX[(b >> 4) as usize] as char);
                            hex.push(HEX[(b & 0xf) as usize] as char);
                        }
                        None => hex.push('?'),
                    }
                }
            }
            crate::pr_info!(
                "trap: #UD at rip={:#x} pid {} comm {} bytes {}",
                regs.rip,
                pid,
                comm,
                hex
            );
        }
        if sigill_has_handler() && !sigsegv_blocked_current() {
            let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGILL as i32);
            crate::signal::force_unblock_current(crate::signal::Signal::SIGILL as i32);
            return;
        }
        let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGILL as i32);
        crate::process::exit::do_exit(-(crate::signal::Signal::SIGILL as i32));
        // Do NOT advance rip — the task is ZOMBIE and will not resume
    } else {
        // A kernel-mode #UD is always a kernel bug; skipping it would
        // corrupt execution. The Oops path prints the report and
        // applies the policy (Tranche 5, the riscv64 twin's).
        #[cfg(feature = "x86_64")]
        crate::dfx::scribble::crash_report(b"#UD", regs);
        crate::dfx::oops::oops(
            crate::dfx::oops::Kind::IllegalInstruction,
            &|w: &mut crate::dfx::backtrace::ConsoleWriter| {
                oops_show_regs(w, regs);
            },
        );
    }
}

/// SIGILL (4) disposition check for the #UD path (processdump/CFI crash
/// catchers in OH install one).
fn sigill_has_handler() -> bool {
    crate::sched::current()
        .and_then(|t| (*t).signal.as_ref().map(|s| s.get_action(4)))
        .map(|a| a.is_some_and(|a| a.has_handler()))
        .unwrap_or(false)
}

/// Handle #GP: user mode → SIGSEGV (Linux semantics), kernel → bug.
fn handle_general_protection(regs: &mut PtRegs) {
    // OH bring-up: name the faulting task — b85's repeating user #GP at
    // ld.so+0xba256 could not be attributed to any pid.
    crate::pr_info!("trap: #GP at rip={:#x}, mode={}, pid {}",
        regs.rip, if regs.user_mode() { "user" } else { "kernel" },
        crate::process::current_pid());

    if regs.user_mode() {
        // Handler-first routing (same as the page-fault Segfault path):
        // debuggers/crash catchers must see the SIGSEGV; the default
        // disposition kills here. A fault inside the (blocked) SIGSEGV
        // handler itself is forced to the default action — unblocking
        // alone would re-enter the handler and loop forever.
        let pid = crate::process::current_pid();
        if sigsegv_has_handler() && !sigsegv_blocked_current() {
            let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGSEGV as i32);
            // Synchronous fault: deliver despite a blocked mask
            // (Linux force_sig parity — see force_unblock_current).
            crate::signal::force_unblock_current(crate::signal::Signal::SIGSEGV as i32);
            return;
        }
        let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGSEGV as i32);
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

/// Arithmetic exceptions: #DE / #OF / #BR / #MF / #XM → SIGFPE.
fn handle_arith_exception(regs: &mut PtRegs, vector: u64) {
    crate::pr_debug!("trap: arith exception {:#x} at rip={:#x}", vector, regs.rip);
    if regs.user_mode() {
        // Linux delivers SIGFPE (catchable — fenv/overflow tests install
        // handlers); only the default disposition kills the task.
        let pid = crate::process::current_pid();
        let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGFPE as i32);
        if !sigfpe_has_handler() {
            crate::process::exit::do_exit(-(crate::signal::Signal::SIGFPE as i32));
        }
    } else {
        panic!("trap: arith exception {:#x} in kernel mode at rip={:#x}", vector, regs.rip);
    }
}

/// #AC (alignment check) → SIGBUS in user mode.
fn handle_alignment_check(regs: &mut PtRegs) {
    if regs.user_mode() {
        let pid = crate::process::current_pid();
        let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGBUS as i32);
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

/// True when the CURRENT task blocks SIGSEGV (bit 10) — the thread is
/// inside a SIGSEGV handler (delivery blocks the signal unless
/// SA_NODEFER) or blocked it via sigprocmask. Linux force_sig_info
/// semantics: a FORCED signal that is blocked is reset to SIG_DFL and
/// unblocked — i.e. a synchronous fault on a blocked SIGSEGV kills the
/// task. Merely pending it and iretq-ing back to the faulting rip
/// refaults forever: OH's DFX_SignalHandler itself faults reading past
/// its thread-stack top, and the redelivery loop starved the whole
/// machine (96k+ identical faults in one boot).
fn sigsegv_blocked_current() -> bool {
    crate::sched::current()
        .map(|t| (*t).sigmask & (1u64 << 10) != 0)
        .unwrap_or(false)
}

fn sigbus_has_handler() -> bool {
    crate::sched::current()
        .and_then(|t| (*t).signal.as_ref().map(|s| s.get_action(7)))
        .map(|a| a.is_some_and(|a| a.has_handler()))
        .unwrap_or(false)
}

/// SIGFPE (8) disposition check for the arith-exception path.
fn sigfpe_has_handler() -> bool {
    crate::sched::current()
        .and_then(|t| (*t).signal.as_ref().map(|s| s.get_action(8)))
        .map(|a| a.is_some_and(|a| a.has_handler()))
        .unwrap_or(false)
}

/// Fatal user-mode SIGSEGV forensics: the GP register set, the VMA
/// covering the faulting rip, and the first stack qwords (return
/// addresses) — enough to identify which mapping jumped through a
/// garbage pointer (the gnome-shell ELF-magic wild-jump class:
/// fault == 0x010102464c457f == LE bytes "\x7fELF\x02\x01\x01\x00").
/// Diagnostic only: every read is checked through the page-table walk.
fn user_segv_forensics(regs: &PtRegs, fault_addr: u64) {
    let pid = crate::sched::get_current_pid();
    // sc112: name the crashing process — the memfd USB DDK PnP NULL-walk
    // kills an unnamed samgr SA host; comm decides which image cfg to cut.
    let comm = crate::sched::current()
        .map(|t| unsafe { (*t).comm() })
        .map(|c| core::str::from_utf8(c).unwrap_or(""))
        .unwrap_or("")
        .trim_end_matches('\0');
    // sc160: the crash-retry loop (AMS respawns the launcher, every child
    // dies at the same stale VA) turned this ~21-line report into a 14
    // k-line serial storm — and each printk holds the UART irqsave lock,
    // so the storm itself manufactured the 453-1000 ms IRQ-OFF windows
    // that kill foundation via its XCollie watchdog (681 reports in
    // sc159). Full forensics for the first 6 faults; after that a
    // one-liner that keeps the census and adds the one attribution that
    // matters: whether cr2 falls inside a live binder buffer mapping
    // (the range forked children lost with VmaFlags DONTFORK in sc157).
    //
    // sc254 (leg-19): the one-liner also names the fault VMA's fd/path/
    // accmode when it is FileBacked (one VMA lookup + one fdtable read —
    // no register dump, no nearvma walk), and the FULL-dump budget is
    // PER-COMM (first 2 per comm, 8 comm slots) AFTER the global 8: the
    // global budget was eaten by the settingsdata crash family before
    // any launcher instance faulted, so the launcher's fd-40-class VMA
    // never got its dump in sc254.
    let binder_hit = crate::ipc::binder::binder_mmap_owner_of(fault_addr);
    {
        static SEGVD_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        // Per-comm budget table (leg-19): cheap FNV-1a of the first 8
        // comm bytes; a match counts against that comm's 2-dump budget.
        static COMM_H: [core::sync::atomic::AtomicU64; 8] =
            [const { core::sync::atomic::AtomicU64::new(0) }; 8];
        static COMM_N: [core::sync::atomic::AtomicUsize; 8] =
            [const { core::sync::atomic::AtomicUsize::new(0) }; 8];
        let mut ch: u64 = 0xcbf29ce484222325;
        for &b in comm.as_bytes().iter().take(8) {
            ch ^= b as u64;
            ch = ch.wrapping_mul(0x100000001b3);
        }
        if ch == 0 {
            ch = 1;
        }
        let mut slot = usize::MAX;
        for i in 0..8 {
            let s = COMM_H[i].load(core::sync::atomic::Ordering::Relaxed);
            if s == ch {
                slot = i;
                break;
            }
            if s == 0
                && slot == usize::MAX
                && COMM_H[i]
                    .compare_exchange(
                        0,
                        ch,
                        core::sync::atomic::Ordering::Relaxed,
                        core::sync::atomic::Ordering::Relaxed,
                    )
                    .is_ok()
            {
                slot = i;
                break;
            }
        }
        let per_comm_ok = slot != usize::MAX
            && COMM_N[slot].fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 2;
        if SEGVD_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) >= 8 && !per_comm_ok {
            // Fault-VMA fd attribution (leg-19): works at ANY budget
            // depth, so the launcher crash-loop names its fd on every
            // instance.
            let mut fdline = alloc::string::String::new();
            if let Some(aspace) =
                crate::sched::current().and_then(|t| t.address_space()).as_ref()
            {
                let vmas = aspace.vma_read();
                if let Some(v) =
                    vmas.find(crate::mm::page::VirtAddr::new(fault_addr as usize))
                {
                    let fd = v.file_fd();
                    if fd >= 0 {
                        // SAFETY: diagnostic read of the fd's file.
                        match unsafe { crate::fs::file::get_file_fd(fd as usize) } {
                            Some(f) => {
                                use core::fmt::Write;
                                let _ = write!(
                                    fdline,
                                    " fdVMA fd={} acc={:#x} off={:#x} flags={:#x} {}",
                                    fd,
                                    f.flags().bits() & 0o3,
                                    v.offset(),
                                    v.flags().bits(),
                                    f.path()
                                );
                            }
                            None => {
                                // fd closed: name the VMA's pinned file.
                                if let Some(f) =
                                    aspace.get_vma_file(v.start().as_usize())
                                {
                                    use core::fmt::Write;
                                    let _ = write!(
                                        fdline,
                                        " fdVMA fd={} closed pin={} acc={:#x} off={:#x} flags={:#x}",
                                        fd,
                                        f.path(),
                                        f.flags().bits() & 0o3,
                                        v.offset(),
                                        v.flags().bits()
                                    );
                                }
                            }
                        }
                    }
                }
            }
            crate::pr_err!(
                "USEGV-L pid={} comm='{}' rip={:#x} cr2={:#x} {}{}",
                pid,
                comm,
                regs.rip,
                fault_addr,
                match binder_hit.as_ref() {
                    Some((proc_id, owner_pid, base, size)) => alloc::format!(
                        "BINDERBUF proc={} owner_pid={} base={:#x} size={:#x}",
                        proc_id,
                        owner_pid,
                        base,
                        size
                    ),
                    None => alloc::string::String::new(),
                },
                fdline
            );
            return;
        }
    }
    if let Some((proc_id, owner_pid, base, size)) = binder_hit.as_ref() {
        crate::pr_err!(
            "USEGV-FOR: cr2 {:#x} INSIDE BINDER BUFFER of proc {} (owner pid {}) base {:#x} size {:#x} — stale daemon mapping dropped at fork (DONTFORK)",
            fault_addr, proc_id, owner_pid, base, size
        );
    }
    crate::pr_err!(
        "USEGV-FOR: pid={} comm='{}' rip={:#x} cr2={:#x} rsp={:#x} rbp={:#x}",
        pid, comm, regs.rip, fault_addr, regs.rsp, regs.rbp
    );
    crate::pr_err!(
        "USEGV-FOR: rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x} rsi={:#x} rdi={:#x}",
        regs.rax, regs.rbx, regs.rcx, regs.rdx, regs.rsi, regs.rdi
    );
    crate::pr_err!(
        "USEGV-FOR: r8={:#x} r9={:#x} r10={:#x} r11={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x}",
        regs.r8, regs.r9, regs.r10, regs.r11, regs.r12, regs.r13, regs.r14, regs.r15
    );

    // VMA covering the faulting rip (which library / JIT region ran).
    let addr_space = crate::sched::current().and_then(|t| t.address_space());
    if let Some(aspace) = addr_space.as_ref() {
        let vmas = aspace.vma_read();
        // leg-27: also name the INDIRECT-CALL TARGET (rax at a CFI
        // slowpath fault = the callee whose shadow slice was touched)
        // and the stack VMA (rsp inside a file-backed read-only segment
        // would name a clobbered stack). Print-path only.
        for &probe in &[regs.rip, fault_addr, regs.rax, regs.rsp] {
            match vmas.find(crate::mm::page::VirtAddr::new(probe as usize)) {
                Some(v) => crate::pr_err!(
                    "USEGV-FOR: vma({:#x}) = {:#x}-{:#x} flags={:#x} type={:?} fd={} off={:#x} origin={}{}",
                    probe,
                    v.start().as_usize(),
                    v.end().as_usize(),
                    v.flags().bits(),
                    v.vma_type(),
                    v.file_fd(),
                    v.offset(),
                    v.origin().creator_name(),
                    if v.origin().is_forked() { "+FORK" } else { "" }
                ),
                None => crate::pr_err!("USEGV-FOR: vma({:#x}) = NONE", probe),
            }
        }
        // sc252 probe (leg-18): for the FAULT-ADDRESS VMA, name the
        // backing fd — path, open mode, inode size — plus every sibling
        // VMA mapping the SAME fd in this mm. The launcher crash-loop
        // writes into a READ|PRIVATE file VMA (fd 40); whether that fd
        // is O_RDONLY (app-side fallback after an EACCES) or O_RDWR
        // (kernel flag bug) is decided by this one line.
        if let Some(v) = vmas.find(crate::mm::page::VirtAddr::new(fault_addr as usize)) {
            let fd = v.file_fd();
            if fd >= 0 {
                match unsafe { crate::fs::file::get_file_fd(fd as usize) } {
                    Some(f) => {
                        let (mode, size) = unsafe {
                            let inode_opt = &*f.inode.get();
                            inode_opt
                                .as_ref()
                                .map(|i| (i.mode.bits(), i.get_size()))
                                .unwrap_or((0, 0))
                        };
                        crate::pr_err!(
                            "USEGV-FOR: cr2fd fd={} path={} accmode={:#x} rawflags={:#x} mode={:#o} size={}",
                            fd,
                            f.path(),
                            f.flags().bits() & 0o3,
                            f.flags().bits(),
                            mode,
                            size
                        );
                        // Siblings: other VMAs of the same fd (an RW|SHARED
                        // twin would prove the app mapped the file twice
                        // and wrote through the wrong one).
                        let mut sibs = 0usize;
                        for sv in vmas.iter() {
                            if sv.file_fd() == fd
                                && sv.start().as_usize() != v.start().as_usize()
                            {
                                if sibs >= 4 {
                                    crate::pr_err!("USEGV-FOR: cr2fd-sib ... (capped)");
                                    break;
                                }
                                crate::pr_err!(
                                    "USEGV-FOR: cr2fd-sib {:#x}-{:#x} flags={:#x} off={:#x}",
                                    sv.start().as_usize(),
                                    sv.end().as_usize(),
                                    sv.flags().bits(),
                                    sv.offset()
                                );
                                sibs += 1;
                            }
                        }
                    }
                    None => {
                        crate::pr_err!(
                            "USEGV-FOR: cr2fd fd={} path=? (fd closed; vma_files pin)",
                            fd
                        );
                        // The VMA's pinned file survives fd close — name it.
                        if let Some(f) = aspace.get_vma_file(v.start().as_usize()) {
                            crate::pr_err!(
                                "USEGV-FOR: cr2fd-pin path={} accmode={:#x}",
                                f.path(),
                                f.flags().bits() & 0o3
                            );
                        }
                    }
                }
            }
            // leg-21 probe (cr2elf): the launcher crash-loop writes a
            // refcount lock into the first PT_LOAD of a memfd-backed
            // module. The one comparison that decides "kernel VMA
            // construction vs broken module content" is the module's OWN
            // program headers: if the file marks the faulting file-offset
            // writable, our mmap dropped a requested prot bit; if the file
            // itself marks it read-only, the pointer is wrong on the
            // userspace side (generator layout or relocation base). The
            // offset-0 mapping of the same fd carries the ELF header and
            // phdrs at mapping start — read them through the page-table
            // walk (present pages of a read-only segment are the file's
            // own bytes; ld.so never COWs them).
            if let Some(v0) = if v.offset() == 0 {
                Some(v)
            } else {
                vmas
                    .iter()
                    .find(|sv| sv.file_fd() == fd && sv.offset() == 0)
            } {
                let base0 = v0.start().as_usize() as u64;
                let word = |o: u64| fore_read_u64(aspace.root_ppn(), base0 + o);
                let foff = (v.offset() + (fault_addr as usize - v.start().as_usize())) as u64;
                if let Some(w0) = word(0) {
                    if w0 & 0xffff_ffff == 0x464c_457f && (w0 >> 32) & 0xff == 2 {
                        let e_type = (word(16).unwrap_or(0)) & 0xffff;
                        let phoff = word(32).unwrap_or(0);
                        let e_phentsize = ((word(48).unwrap_or(0)) >> 48) & 0xffff;
                        let e_phnum = ((word(56).unwrap_or(0)) & 0xffff) as usize;
                        let entsz = if e_phentsize == 0 { 56 } else { e_phentsize as u64 };
                        crate::pr_err!(
                            "USEGV-FOR: cr2elf type={:#x} phoff={:#x} phnum={} foff={:#x} (fault file offset)",
                            e_type, phoff, e_phnum, foff
                        );
                        for i in 0..e_phnum.min(12) {
                            let po = phoff + (i as u64) * entsz;
                            let (pt, pf) = match word(po) {
                                Some(w) => (w & 0xffff_ffff, (w >> 32) & 0xffff_ffff),
                                None => break,
                            };
                            let p_off = word(po + 8).unwrap_or(0);
                            let p_va = word(po + 16).unwrap_or(0);
                            let p_fsz = word(po + 32).unwrap_or(0);
                            let p_msz = word(po + 40).unwrap_or(0);
                            // PT_LOAD(1) / PT_DYNAMIC(2) / PT_GNU_RELRO.
                            if pt == 1 || pt == 2 || pt == 0x6474_e552 {
                                let covers = pt == 1
                                    && p_off <= foff
                                    && foff < p_off.saturating_add(p_fsz);
                                crate::pr_err!(
                                    "USEGV-FOR: cr2elf ph{} type={:#x} flags={:#x} off={:#x} vaddr={:#x} filesz={:#x} memsz={:#x}{}",
                                    i, pt, pf, p_off, p_va, p_fsz, p_msz,
                                    if covers { " <= FAULT" } else { "" }
                                );
                            }
                        }
                    }
                }
                // 32 bytes around the fault address: an initialized
                // refcount control block vs zeros vs garbage.
                {
                    let mut line = alloc::string::String::from("USEGV-FOR: cr2bytes");
                    use core::fmt::Write;
                    for k in 0..4u64 {
                        let va = (fault_addr as u64 & !0xf) + k * 8;
                        let _ = match fore_read_u64(aspace.root_ppn(), va) {
                            Some(w) => write!(line, " {:#x}:{:016x}", va, w),
                            None => write!(line, " {:#x}:????", va),
                        };
                    }
                    crate::pr_err!("{}", line);
                }
                // Printable runs in the module's first pages: .dynstr
                // SONAME/symbol names name the module even when the fd is
                // an anonymous memfd (compare against the on-disk .so to
                // decide copy-vs-generated).
                {
                    let mut line = alloc::string::String::from("USEGV-FOR: cr2strs");
                    use core::fmt::Write;
                    let mut hits = 0usize;
                    let mut run = alloc::vec::Vec::new();
                    let mut va = base0;
                    let end = base0 + 0x8000;
                    'scan: while va < end {
                        match fore_read_u64(aspace.root_ppn(), va) {
                            Some(w) => {
                                for k in 0..8usize {
                                    let b = (w >> (k * 8)) & 0xff;
                                    if (0x20..0x7f).contains(&b) {
                                        run.push(b as u8);
                                    } else {
                                        if run.len() >= 10 && hits < 10 {
                                            let _ = write!(
                                                line,
                                                " [{}]",
                                                core::str::from_utf8(&run).unwrap_or("?")
                                            );
                                            hits += 1;
                                        }
                                        run.clear();
                                        if hits >= 10 {
                                            break 'scan;
                                        }
                                    }
                                }
                            }
                            None => break 'scan,
                        }
                        va += 8;
                    }
                    crate::pr_err!("{}", line);
                }
            }
        }
        // File path + module-relative fault offset for the rip VMA, so a
        // crash inside a mapped .so names the library and gives the
        // addr2line offset directly (sc49: appspawn children die inside a
        // FileBacked VMA and fd/offset alone leaves the crash site
        // anonymous).
        if let Some(v) = vmas.find(crate::mm::page::VirtAddr::new(regs.rip as usize)) {
            let fd = v.file_fd();
            if fd >= 0 {
                // SAFETY: current task's fd table is alive for the
                // duration of the fault report.
                let path = unsafe { crate::fs::file::get_file_fd(fd as usize) }
                    .map(|f| f.path())
                    .unwrap_or_else(|| alloc::string::String::from("?"));
                crate::pr_err!(
                    "USEGV-FOR: ripfile {} voff {:#x}",
                    path,
                    v.offset() + (regs.rip as usize - v.start().as_usize())
                );
                // sc106 OsAccount hunt: the recurring SA-host crash
                // (identical rip, NULL list walk) runs inside a memfd-
                // hosted library the offline tooling cannot name. Its
                // .dynstr/.rodata usually sits in the read-only segment
                // mapped directly below the r-x segment — scan up to
                // 64 KiB below the mapping for printable runs so the
                // crash site names its library (unmapped gaps stop the
                // scan safely; page-table walks never fault).
                if path.contains("anon") {
                    let mut line = alloc::string::String::from("USEGV-FOR: memfd-strs");
                    use core::fmt::Write;
                    let mut hits = 0usize;
                    let mut run = alloc::vec::Vec::new();
                    let start_va = (v.start().as_usize() as u64).saturating_sub(0x1_0000);
                    let end_va = v.start().as_usize() as u64;
                    let mut va = start_va;
                    'scan: while va < end_va {
                        match fore_read_u64(aspace.root_ppn(), va) {
                            Some(w) => {
                                for k in 0..8usize {
                                    let b = (w >> (k * 8)) & 0xff;
                                    if (0x20..0x7f).contains(&b) {
                                        run.push(b as u8);
                                    } else {
                                        if run.len() >= 10 && hits < 8 {
                                            let _ = write!(
                                                line,
                                                " [{}]",
                                                core::str::from_utf8(&run)
                                                    .unwrap_or("?")
                                            );
                                            hits += 1;
                                        }
                                        run.clear();
                                        if hits >= 8 {
                                            break 'scan;
                                        }
                                    }
                                }
                            }
                            None => {
                                // Unmapped gap — skip 4 KiB ahead.
                                va = (va & !0xfff) + 0x1000;
                                continue;
                            }
                        }
                        va += 8;
                    }
                    crate::pr_err!("{}", line);
                }
            }
        }
        // Instruction bytes at rip: the dynamic linker CLOSES the mmap fd
        // after mapping, so the path is often unrecoverable from the VMA —
        // the bytes identify the exact function offline (match against the
        // .so files extracted from the system/vendor images).
        {
            let mut bytes = [0u8; 16];
            let mut ok = true;
            for (i, b) in bytes.iter_mut().enumerate() {
                match fore_read_u64(aspace.root_ppn(), regs.rip + i as u64) {
                    Some(w) => *b = (w & 0xff) as u8,
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                crate::pr_err!(
                    "USEGV-FOR: ripbytes {:02x?}",
                    bytes
                );
            }
        }
        // VMA neighborhood around the fault address (render_service
        // offset-anomaly family, session 18): the single vma(cr2) line
        // cannot show what borders the faulting region — whether a
        // writable stack sits directly below a mis-flagged PROT_NONE
        // tail, a guard page, or a hole. sc252 (leg-18): the old
        // ascending walk with a 12-entry cap spent every entry on the
        // huge module mappings tens of MB BELOW cr2 and never reached
        // the faulting VMA's own neighbors (the launcher lock-region
        // dump was cut at 0x7ff0817c0000, 2.5MB below the fault).
        // Collect the window first, then print the 12 entries NEAREST
        // cr2 (distance from the fault page), so the immediate
        // neighborhood always makes the budget.
        {
            let lo = (fault_addr as usize).saturating_sub(0x100_0000);
            let hi = (fault_addr as usize).saturating_add(0x100_0000);
            let mut window: alloc::vec::Vec<&crate::mm::vma::Vma> = alloc::vec::Vec::new();
            for v in vmas.iter() {
                if v.end().as_usize() <= lo || v.start().as_usize() >= hi {
                    continue;
                }
                window.push(v);
            }
            window.sort_by_key(|v| {
                v.start()
                    .as_usize()
                    .abs_diff(fault_addr as usize)
            });
            let mut shown = 0usize;
            for v in window {
                if shown >= 12 {
                    crate::pr_err!("USEGV-FOR: nearvma ... (capped)");
                    break;
                }
                crate::pr_err!(
                    "USEGV-FOR: nearvma {:#x}-{:#x} flags={:#x} type={:?} fd={} off={:#x} origin={}{}",
                    v.start().as_usize(),
                    v.end().as_usize(),
                    v.flags().bits(),
                    v.vma_type(),
                    v.file_fd(),
                    v.offset(),
                    v.origin().creator_name(),
                    if v.origin().is_forked() { "+FORK" } else { "" }
                );
                shown += 1;
            }
        }
    }

    // Stack words (return-address chain): read through the page-table
    // walk so a bad/absent rsp cannot take the kernel down.
    fn fore_read_u64(root_ppn: u64, va: u64) -> Option<u64> {
        // SAFETY: read-only page-table walk on a live root.
        let phys = unsafe {
            crate::arch::mm::mm_ops::PageTableWalker::walk(root_ppn, va & !0xfff)?
        };
        let kv = crate::arch::mm::memory_layout::phys_to_virt(
            crate::arch::mm::memory_layout::PhysAddr::new((phys.0 << 12) | (va & 0xfff)),
        );
        // SAFETY: kv points at the mapped user page through the linear map.
        Some(unsafe { core::ptr::read_volatile(kv.as_u64() as *const u64) })
    }
    if let Some(aspace) = addr_space.as_ref() {
        let root = aspace.root_ppn();
        let base = (regs.rsp & !0xf) as u64;
        let mut line = alloc::string::String::new();
        for i in 0..8usize {
            // sc260 (leg-23): the walk addition is checked — a garbage
            // rsp near u64::MAX must not overflow-panic the fault
            // handler. forensics may report garbage, never die on it.
            let va = match base.checked_add((i * 8) as u64) {
                Some(v) => v,
                None => break,
            };
            let val = fore_read_u64(root, va)
                .map(|v| alloc::format!("{:#x}", v))
                .unwrap_or(alloc::string::String::from("????????????????"));
            use core::fmt::Write;
            let _ = write!(line, " {}:{}", i, val);
        }
        crate::pr_err!("USEGV-FOR: stack@rsp:{}", line);
        // leg-31B: name the VMA (+origin) behind each stack word that
        // looks like a user code pointer — the faulting frame's return
        // addresses name the CALLER library without a guest-side
        // backtracer (the render_service guard-read fault died inside
        // ld-musl memcpy; the caller's return address was the only
        // attribution clue and it lived on the stack).
        if let Some(vmas2) = addr_space.as_ref().map(|a| a.vma_read()) {
            for i in 0..8usize {
                let va = match base.checked_add((i * 8) as u64) {
                    Some(v) => v,
                    None => break,
                };
                if let Some(val) = fore_read_u64(root, va) {
                    if val <= 0x1000 || val >= 0x0000_8000_0000_0000 {
                        continue;
                    }
                    if let Some(v) = vmas2
                        .find(crate::mm::page::VirtAddr::new(val as usize))
                    {
                        // Only code-looking mappings (r-x / rwx).
                        let f = v.flags();
                        if f.is_executable() {
                            // Name the backing: fd or the pinned vm_file
                            // (memfd-loaded libraries are FileBacked with
                            // an anon_inode path — the pin is the only
                            // name they have).
                            let mut backing =
                                alloc::string::String::new();
                            if let Some(fl) = aspace.get_vma_file(
                                v.start().as_usize(),
                            ) {
                                use core::fmt::Write;
                                let _ = write!(
                                    backing,
                                    " path={}",
                                    fl.path()
                                );
                            } else if v.file_fd() >= 0 {
                                use core::fmt::Write;
                                let _ = write!(
                                    backing,
                                    " fd={}",
                                    v.file_fd()
                                );
                            }
                            crate::pr_err!(
                                "USEGV-FOR: stkva[{}] {:#x} = {:#x}-{:#x} r-x off={:#x}{} origin={}{}",
                                i,
                                val,
                                v.start().as_usize(),
                                v.end().as_usize(),
                                v.offset(),
                                backing,
                                v.origin().creator_name(),
                                if v.origin().is_forked() { "+FORK" } else { "" }
                            );
                            // sc324 follow-up: code bytes at the return
                            // site (16 before, 8 after) read through the
                            // page-table walk — fingerprints the caller
                            // even for memfd-backed libraries with no
                            // on-disk copy to compare against.
                            let mut cb = alloc::string::String::new();
                            use core::fmt::Write;
                            let _ = write!(cb, "USEGV-FOR: stkcode[{}]", i);
                            let lo = (val - 16) & !7;
                            for k in 0..3u64 {
                                match fore_read_u64(root, lo + k * 8) {
                                    Some(w) => {
                                        let _ = write!(cb, " {:#x}", w);
                                    }
                                    None => {
                                        let _ = write!(cb, " :?");
                                    }
                                }
                            }
                            crate::pr_err!("{}", cb);
                        }
                    }
                }
            }
        }
    }
    // leg-21 probe: the launcher crash family dies inside the dynamic
    // linker's pthread_cond walk, whose frame parks the condvar pointer
    // at rbp-0x58. Dump the rbp locals window plus a dereference of the
    // rbp-0x58 slot when it holds a canonical user pointer — the
    // condvar's own words (queue head/tail/lock/seq) are the corrupted
    // state the queue walk followed.
    if let Some(aspace) = addr_space.as_ref() {
        let root = aspace.root_ppn();
        // sc260 (leg-23): the probe runs on ARBITRARY user register
        // garbage. This exact boot died at the tail: pid 526's memcpy
        // crash carried rbp=0xb, `rbp - 0x58` underflowed, and the
        // debug-overflow panic turned an adjudicated userspace SEGV
        // into a kernel death (PANIC: attempt to subtract with
        // overflow, trap.rs:2340). The window only opens when rbp is
        // a canonical user pointer — an rbp too small to subtract
        // 0x58 from (or a kernel-space rbp, which the user-root walk
        // could never read anyway) simply has no frame to dump.
        if regs.rbp > 0x1000 && regs.rbp < 0x0000_8000_0000_0000 {
            if let Some(w) = fore_read_u64(root, regs.rbp.wrapping_sub(0x58)) {
                let mut line = alloc::string::String::new();
                use core::fmt::Write;
                let _ = write!(line, "USEGV-FOR: frame rbp-0x58={:#x}", w);
                if w > 0x1000 && w < 0x0000_8000_0000_0000 {
                    for i in 0..6usize {
                        let va = match w.checked_add((i * 8) as u64) {
                            Some(v) => v,
                            None => break,
                        };
                        let _ = match fore_read_u64(root, va) {
                            Some(v) => write!(line, " {:#x}:{:#x}", va, v),
                            None => write!(line, " {:#x}:????", va),
                        };
                    }
                }
                crate::pr_err!("{}", line);
            }
        }
    }

    // leg-22 probes (sc258 verdict: libhicollie.z.so, condvar at
    // module+0x3c108 inside the watchdog static at module+0x3bff0, its
    // _c_head slot (obj+0x140) holds a pointer to the module's own
    // .rodata string at module+0xa92c). In musl's cond apply() the
    // condvar arrives in rdi and survives in r9; r15 carries the
    // hicollie object base (c-0x118). Three dumps decide the fix
    // direction:
    //   cv       — the condvar's own words (tail@+8, waiters@+0xc,
    //              seq@+0x10, lock@+0x20, head@+0x28): a single stale
    //              field vs whole-object garbage.
    //   obj      — watchdog-object fields around the condvar
    //              (+0x100..+0x170, plus +0x2f0..+0x320): fresh
    //              constructor values vs foreign patterns.
    //   bssslack — the anon .bss page's tail PAST the segment end
    //              (ph4 memsz ends at module+0x3c51c; the page runs to
    //              0x3c600). Nothing in the module addresses that
    //              slack, so ANY nonzero word there means the page
    //              arrived dirty from the recycler — the kernel-bug
    //              smoking gun. module base = cr2 - 0xa93c (the fault
    //              foff for this exact crash family).
    if let Some(aspace) = addr_space.as_ref() {
        let root = aspace.root_ppn();
        let is_user_ptr = |p: u64| p > 0x1000 && p < 0x0000_8000_0000_0000;
        // cv words via r9 (condvar) when it is the apply()-shape:
        // rdi == r9 + 0x20 (the re-lock slot).
        if is_user_ptr(regs.r9) && regs.rdi == regs.r9 + 0x20 {
            let mut line = alloc::string::String::new();
            use core::fmt::Write;
            let _ = write!(line, "USEGV-FOR: cv r9={:#x}", regs.r9);
            for i in 0..8usize {
                let va = regs.r9 + (i * 8) as u64;
                let _ = match fore_read_u64(root, va) {
                    Some(v) => write!(line, " +{:#x}:{:#x}", i * 8, v),
                    None => write!(line, " +{:#x}:????", i * 8),
                };
            }
            crate::pr_err!("{}", line);
            // obj window: the hicollie watchdog object is at c-0x118
            // (r15 usually already holds it). wrapping_add: r15 is
            // register garbage on non-apply shapes and may sit near
            // u64::MAX (leg-23 hardening — same class as the sc260
            // rbp-0x58 underflow panic).
            let obj = if is_user_ptr(regs.r15) && regs.r15.wrapping_add(0x118) == regs.r9 {
                regs.r15
            } else {
                regs.r9.wrapping_sub(0x118)
            };
            let mut line = alloc::string::String::new();
            let _ = write!(line, "USEGV-FOR: obj {:#x}", obj);
            const OFFS: &[u64] = &[
                0x100, 0x108, 0x110, 0x148, 0x150, 0x158, 0x160, 0x168, 0x300, 0x308, 0x310,
            ];
            for &off in OFFS {
                let _ = match fore_read_u64(root, obj + off) {
                    Some(v) => write!(line, " +{:#x}:{:#x}", off, v),
                    None => write!(line, " +{:#x}:????", off),
                };
            }
            crate::pr_err!("{}", line);
            // bss slack: needs the module base. The hicollie R-only
            // ph1 VMA starts at module base with offset 0 — when the
            // fault lands in that exact VMA, base = its start.
            if let Some(v) = aspace.vma_read().find(
                crate::mm::page::VirtAddr::new(fault_addr as usize),
            ) {
                if v.vma_type() == crate::mm::vma::VmaType::FileBacked
                    && v.offset() == 0
                    && v.start().as_usize() & 0xfff == 0
                {
                    let base = v.start().as_usize() as u64;
                    let mut line = alloc::string::String::new();
                    let _ = write!(
                        line,
                        "USEGV-FOR: bssslack base={:#x} (page {:#x})",
                        base,
                        base + 0x3c000
                    );
                    let mut nz = 0u32;
                    for i in 0..14usize {
                        let va = base + 0x3c520 + (i * 8) as u64;
                        let _ = match fore_read_u64(root, va) {
                            Some(w) => {
                                if w != 0 {
                                    nz += 1;
                                }
                                write!(line, " {:#x}:{:#x}", va, w)
                            }
                            None => write!(line, " {:#x}:????", va),
                        };
                    }
                    crate::pr_err!("{} nonzero={}", line, nz);
                    // second window: the fully-slack page at +0x3d000
                    // (the anon bss VMA spans +0x3c000..+0x40000 but
                    // ph4 memsz ends at +0x3c51c — pages +0x3d000,
                    // +0x3e000, +0x3f000 are addressed by nothing).
                    let mut line2 = alloc::string::String::new();
                    let _ = write!(line2, "USEGV-FOR: bssslack2 base={:#x}", base);
                    let mut nz2 = 0u32;
                    for i in 0..8usize {
                        let va = base + 0x3d000 + (i * 8) as u64;
                        let _ = match fore_read_u64(root, va) {
                            Some(w) => {
                                if w != 0 {
                                    nz2 += 1;
                                }
                                write!(line2, " {:#x}:{:#x}", va, w)
                            }
                            None => write!(line2, " {:#x}:????", va),
                        };
                    }
                    crate::pr_err!("{} nonzero={}", line2, nz2);
                }
            }
        }
    }
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
static KP_DUMP_ENTERED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

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

    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::Steptrace) {
        crate::pr_debug!(
            "trap: page fault addr={:#x}, rip={:#x}, ec={:#x}, mode={}",
            fault_addr,
            regs.rip,
            error_code,
            if regs.kernel_mode() { "kernel" } else { "user" }
        );
    }

    // The exceptions:page_fault_user / exceptions:page_fault_kernel
    // tracepoints, recorded at fault entry before the handler runs (the
    // reference's record point). On this arch error_code IS the
    // hardware #PF error word (regs.orig_rax); ip is the faulting rip.
    // One armed() check selects the mode's event; the record marshals
    // three words.
    if crate::trace::EVENT_PAGE_FAULT_USER.armed() || crate::trace::EVENT_PAGE_FAULT_KERNEL.armed() {
        let event = if regs.kernel_mode() {
            &crate::trace::EVENT_PAGE_FAULT_KERNEL
        } else {
            &crate::trace::EVENT_PAGE_FAULT_USER
        };
        if event.armed() {
            crate::trace::record(
                event,
                [fault_addr, regs.rip, error_code, 0, 0, 0, 0, 0, 0, 0],
            );
        }
    }

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
                // A blocked SIGSEGV (fault inside its own handler) is
                // forced to the default action instead of pended — see
                // sigsegv_blocked_current.
                user_segv_forensics(regs, fault_addr);
                let pid = crate::process::current_pid();
                if sigsegv_has_handler() && !sigsegv_blocked_current() {
                    let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGSEGV as i32);
                    return;
                }
                let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGSEGV as i32);
                crate::process::exit::do_exit(-(crate::signal::Signal::SIGSEGV as i32));
            } else {
                #[cfg(feature = "x86_64")]
                crate::dfx::scribble::crash_report(b"#PF-segv", regs);
                // The riscv64 mm layer returns KernelPanic for kernel
                // faults; until the x86 twin grows that, do NOT silently
                // iretq back into the faulting rip (infinite fault loop).
                // The Oops path prints the report and applies the policy
                // (panic in interrupt context / oops=panic / idle, task
                // death otherwise — Tranche 5).
                crate::dfx::oops::oops(
                    crate::dfx::oops::Kind::PageFault { addr: fault_addr },
                    &|w: &mut crate::dfx::backtrace::ConsoleWriter| {
                        oops_show_regs(w, regs);
                    },
                );
            }
        }
        MmFaultResult::PermissionDenied => {
            crate::pr_err!("pagefault: Permission denied at {:#x}, rip={:#x}, pid={}",
                fault_addr, regs.rip, crate::sched::get_current_pid());
            if regs.user_mode() {
                // Handler-first routing like Segfault (PROT_NONE access);
                // likewise force-default a blocked SIGSEGV.
                user_segv_forensics(regs, fault_addr);
                let pid = crate::process::current_pid();
                if sigsegv_has_handler() && !sigsegv_blocked_current() {
                    let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGSEGV as i32);
                    return;
                }
                let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGSEGV as i32);
                crate::process::exit::do_exit(-(crate::signal::Signal::SIGSEGV as i32));
            } else {
                #[cfg(feature = "x86_64")]
                crate::dfx::scribble::crash_report(b"#PF-perm", regs);
                // Oops path (Tranche 5): report + policy, like the
                // Segfault arm above.
                crate::dfx::oops::oops(
                    crate::dfx::oops::Kind::PageFault { addr: fault_addr },
                    &|w: &mut crate::dfx::backtrace::ConsoleWriter| {
                        oops_show_regs(w, regs);
                    },
                );
            }
        }
        MmFaultResult::BusError => {
            crate::pr_err!("pagefault: Bus error (past EOF) at {:#x}", fault_addr);
            if regs.user_mode() {
                let pid = crate::process::current_pid();
                let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGBUS as i32);
                if !sigbus_has_handler() {
                    crate::process::exit::do_exit(-(crate::signal::Signal::SIGBUS as i32));
                }
            } else {
                #[cfg(feature = "x86_64")]
                crate::dfx::scribble::crash_report(b"#PF-bus", regs);
                // Oops path (Tranche 5): report + policy, like the
                // Segfault arm above.
                crate::dfx::oops::oops(
                    crate::dfx::oops::Kind::PageFault { addr: fault_addr },
                    &|w: &mut crate::dfx::backtrace::ConsoleWriter| {
                        oops_show_regs(w, regs);
                    },
                );
            }
        }
        MmFaultResult::OutOfMemory => {
            crate::pr_err!("pagefault: Out of memory at {:#x}", fault_addr);
            crate::mm::page_alloc::oom_forensic_dump("pagefault OOM");
            if regs.user_mode() {
                // fake-OOM family: Linux pagefault_out_of_memory parity —
                // give the global OOM killer (badness-based victim
                // selection, rate-limited) the chance to free the real
                // hog before this task dies. Retry on true, die on false
                // (struck out or nothing killable).
                if !crate::mm::oom_kill::fault_oom_kill_or_retry(
                    crate::process::current_pid(),
                ) {
                    crate::process::exit::do_exit(-(crate::signal::Signal::SIGKILL as i32));
                }
                // true: fall through — returning to userspace re-executes
                // the faulting instruction; the victim's freed pages serve
                // the retry.
            } else {
                panic!(
                    "pagefault: OOM in kernel mode at {:#x}, rip={:#x}",
                    fault_addr, regs.rip
                );
            }
        }
        MmFaultResult::KernelPanic => {
            // R73 (sc195 strand + the coordinator's riscv64 double-fault
            // root cause): the dump below used to read 16 words at rsp
            // with NO bound — a word past the real stack top (boot stack
            // at the image end, or any stack-adjacent hole) faulted back
            // into THIS handler, and the print/read recursion wedged the
            // CPU forever (sc195: cpu0 hardirq-held ~855 s at a stable
            // inb rip — the recursive dump's UART output path — while the
            // system limped on cpu1). Two guards:
            // - re-entrancy latch: a fault DURING the dump goes straight
            //   to the halt loop with one marker line (no second dump, no
            //   recursion — the class dies here regardless of which
            //   dump read faulted);
            // - the kp-stk walk is bounded by the task's real
            //   kernel-stack top (16K-aligned rsp heuristic when there is
            //   no task context), stopping at the first past-top word.
            if KP_DUMP_ENTERED.swap(true, core::sync::atomic::Ordering::Relaxed) {
                for &b in b"trap: KERNPANIC re-entered during dump; halting\n" {
                    crate::console::putchar_no_lock(b);
                }
                // SAFETY: hlt halts until the next interrupt; safe in a
                // halt loop.
                loop {
                    unsafe { core::arch::asm!("hlt") };
                }
            }
            // SCRIBBLE2: park hook for gdb (walk the faulting CR3's page
            // tables live from the debugger).
            #[cfg(feature = "x86_64")]
            crate::dfx::scribble::kernpanic_park(fault_addr, regs.rip, regs.rsp);
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
            // OH bring-up forensics: dump 16 kernel-stack words at rsp
            // and walk the frame-pointer chain, raw. When control flow
            // jumps through a garbage pointer (executed stack garbage,
            // use-after-free vtable...) this is the only record of what
            // the stack held at the jump. Words print one per line
            // ("kp-stk"), frames as "kp-frame".
            //
            // R73: the walk is BOUNDED at the faulting task's real
            // kernel-stack top (16K-aligned rsp heuristic when there is
            // no task context). A read past the mapping would fault back
            // into this handler — the exact print/read recursion that
            // wedged sc195's cpu0 for ~855 s; the KP_DUMP_ENTERED latch
            // above is the safety net if any dump read still faults.
            let kstack_top: usize = crate::sched::current()
                .map(|t| unsafe { (*t).kernel_stack_top() })
                .unwrap_or(((regs.rsp as usize | 0x3fff) + 1));
            for i in 0..16usize {
                let addr = regs.rsp as usize + i * 8;
                if addr + 8 > kstack_top {
                    for &b in b"kp-stk <rest past kernel stack top>\n" {
                        crate::console::putchar_no_lock(b);
                    }
                    break;
                }
                // SAFETY: bounded read of our own kernel stack (see the
                // R73 comment above).
                let v = unsafe { core::ptr::read_volatile(addr as *const u64) };
                for &b in b"kp-stk " {
                    crate::console::putchar_no_lock(b);
                }
                kp_put_hex(addr as u64);
                crate::console::putchar_no_lock(b':');
                crate::console::putchar_no_lock(b' ');
                kp_put_hex(v);
                crate::console::putchar_no_lock(b'\n');
            }
            // Frame-pointer walk: [rbp] = next rbp, [rbp+8] = return
            // address. Stops at the stack top or the first bogus frame.
            let stack_top = (regs.rsp | 0x3fff) + 1;
            let mut rbp = regs.rbp;
            for _ in 0..10 {
                if rbp & 7 != 0 || rbp + 16 >= stack_top || rbp < regs.rsp {
                    break;
                }
                // SAFETY: bounds-checked reads of the current kernel stack.
                let (next, ret) = unsafe {
                    (
                        core::ptr::read_volatile(rbp as *const u64),
                        core::ptr::read_volatile((rbp + 8) as *const u64),
                    )
                };
                for &b in b"kp-frame " {
                    crate::console::putchar_no_lock(b);
                }
                kp_put_hex(ret);
                crate::console::putchar_no_lock(b'\n');
                if next <= rbp || next >= stack_top {
                    break;
                }
                rbp = next;
            }
            // Forensic ring tail: the last 12 completed syscalls before
            // the crash (pid/nr/a0/ret), oldest first. Identifies the
            // in-flight syscall for interrupt-time crashes.
            {
                use crate::syscall::dispatch::{SYSCALL_CURSOR, SYSCALL_RING};
                let cur = SYSCALL_CURSOR.load(core::sync::atomic::Ordering::Relaxed);
                let len = SYSCALL_RING.len();
                for k in (0..12usize).rev() {
                    let idx = (cur + len - 1 - k) % len;
                    let e = &SYSCALL_RING[idx];
                    for &b in b"kp-sys " {
                        crate::console::putchar_no_lock(b);
                    }
                    kp_put_hex(e.pid.load(core::sync::atomic::Ordering::Relaxed) as u64);
                    crate::console::putchar_no_lock(b' ');
                    kp_put_hex(e.nr.load(core::sync::atomic::Ordering::Relaxed) as u64);
                    crate::console::putchar_no_lock(b' ');
                    kp_put_hex(e.a0.load(core::sync::atomic::Ordering::Relaxed));
                    crate::console::putchar_no_lock(b' ');
                    kp_put_hex(e.ret.load(core::sync::atomic::Ordering::Relaxed));
                    crate::console::putchar_no_lock(b'\n');
                }
            }
            // The old terminal was an unconditional hlt loop — every
            // kernel page fault with no fixup entry died with the whole
            // machine, captured into nothing. The Oops policy (Tranche
            // 5, the riscv64 twin's) decides instead: Linux-shaped
            // report, pstore capture, then either a real panic
            // (interrupt context / oops=panic / idle) or the faulting
            // task dies with SIGSEGV and the machine keeps running.
            crate::dfx::oops::oops(
                crate::dfx::oops::Kind::PageFault { addr: fault_addr },
                &|w: &mut crate::dfx::backtrace::ConsoleWriter| {
                    oops_show_regs(w, regs);
                },
            );
        }
        _ => {}
    }
}

/// The oops register block — the FAULTING frame in the reference's
/// show_regs layout (arch/x86/kernel/process_64.c + fault.c, Linux
/// 5.15): the RIP line with the symbolized ip, the RSP/EFLAGS line,
/// then the GPRs three per row. The CS column prints 0010/0018 from
/// the frame's segment selectors.
fn oops_show_regs(w: &mut crate::dfx::backtrace::ConsoleWriter, regs: &PtRegs) {
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

const STIME_SLOTS: usize = 24;

struct StimeSample {
    comm: [core::sync::atomic::AtomicU8; 16],
    page: core::sync::atomic::AtomicU64,
    count: core::sync::atomic::AtomicU64,
}

static STIME_HIST: [StimeSample; STIME_SLOTS] = {
    const EMPTY: StimeSample = StimeSample {
        comm: [const { core::sync::atomic::AtomicU8::new(0) }; 16],
        page: core::sync::atomic::AtomicU64::new(0),
        count: core::sync::atomic::AtomicU64::new(0),
    };
    [EMPTY; STIME_SLOTS]
};

/// Focus table (sc106): launcher-comm samples only — see stime_sample_rip.
static STIME_FOCUS: [StimeSample; STIME_SLOTS] = {
    const EMPTY: StimeSample = StimeSample {
        comm: [const { core::sync::atomic::AtomicU8::new(0) }; 16],
        page: core::sync::atomic::AtomicU64::new(0),
        count: core::sync::atomic::AtomicU64::new(0),
    };
    [EMPTY; STIME_SLOTS]
};

/// Record one kernel-mode tick at `rip` for `comm`.
fn stime_sample_rip(rip: u64, comm: &[u8; 16]) {
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
