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

use super::unsafe_edges::SavedFrame;
use super::cpu::{inb, outb, rdmsr, wrmsr};
use crate::arch::pt_regs::{Cause, PtRegs, PT_REGS_SIZE};
use crate::sync::spinlock::Spinlock;

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

/// STAR value: kernel CS 0x08 on SYSCALL; user base 0x23 (the user-code
/// GDT index 6 << 3 | RPL3) so SYSRET64 computes CS = 0x23+16 = 0x33
/// (user code64) and SS = 0x23+8 = 0x2b — both with RPL 3. The base
/// used to be 0x20, which SYSRET would have loaded as CS=0x30/SS=0x28
/// (RPL 0); nothing exercised it while returns were iretq-only
/// (gap-net added the SYSRET tail in trap.S).
const MSR_STAR_VALUE: u64 = (0x23u64 << 48) | (0x08u64 << 32);
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

/// INV-ARCHDIAG-TSS: read-only diagnostic bottom for the arch
/// service crate's foreign-entry report — reads the per-CPU TSS
/// mirror's rsp0 slot (a broken-machine read; the report that consumes
/// it is the point, the CPU parks right after). Safe-typed wrapper over
/// the static-mut mirror; not for healthy-path use.
pub fn diag_tss_rsp0(cpu: usize) -> u64 {
    // SAFETY: read-only u64 read of this CPU's TSS mirror slot; the
    // TSS is written only at context-switch/arm time (set_tss_rsp0),
    // never freed — a torn read names a stale stack top, which the
    // forensic report tolerates by design.
    unsafe { TSS_PER_CPU[cpu].read_u64(TSS_OFF_RSP0) }
}

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
// IRREDUCIBLE F-ROW (F-BOOT-X1): per-CPU GDT/TSS bring-up (static-mut
// per-CPU tables + lgdt/ltr) — reference kernel arch/x86/kernel/cpu/
// common.c cpu_init_exception_handling; boot-section ownership.
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
    // fpu_init and the cpu-state bring-up are safe fns now (their own
    // audited blocks); the stub tables are plain address arrays.
    super::thread::fpu_init();
    init_cpu_state(0);

    // ---- IDT ----
    {
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
            static stub_129: u8;
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

        // SAFETY: single-CPU boot section; the static-mut IDT is
        // exclusively owned here until lidt publishes it.
        let idt = unsafe { &raw mut IDT_X86 };
        for (v, entry) in unsafe { (*idt).iter_mut() }.enumerate() {
            // int1/int3/into are reachable from user mode (DPL 3); #DF
            // uses IST1; everything else is a plain kernel interrupt gate.
            let lv = crate::drivers::intc::apic::LOCAL_TIMER_VECTOR as usize;
            let stub: *const u8 = if v < 48 {
                stubs[v]
            } else if v >= lv && v <= crate::arch::ipi::IPI_LAST as usize {
                apic_stubs[v - lv]
            } else if v == crate::drivers::virtio::MSIX_BLK_VECTOR as usize {
                &raw const stub_129
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
        unsafe { x86_load_idt(&idtr) };

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
            // SAFETY: AP bring-up on its own CPU before enabling
            // interrupts; the shared IDT is read-only here.
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
// IRREDUCIBLE F-ROW (F-BOOT-X2): the 8259 PIC ICW program sequence —
// reference kernel arch/x86/kernel/i8259.c init_8259A; port-IO order
// IS the protocol.
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

/// Serializes PIC mask-register RMWs. Leaf lock (never held across any
/// other acquire). Needed since RT-plan R5: per-line IRQ threads unmask
/// from task context (any CPU) while hardirq top halves mask on the BSP
/// — the old "IF-off or single-owner" contract no longer holds.
static PIC_MASK_LOCK: Spinlock<()> = Spinlock::new(());

/// Unmask one PIC line (0..15).
pub fn unmask_irq_line(line: u8) {
    let _pl = PIC_MASK_LOCK.lock_irqsave();
    // SAFETY: RMW of the mask register, serialized by PIC_MASK_LOCK.
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
    let _pl = PIC_MASK_LOCK.lock_irqsave();
    // SAFETY: RMW of the mask register, serialized by PIC_MASK_LOCK.
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

/// The line owners' names (the irq:irq_handler_entry name field; the
/// x86 PIC path predates the irq-domain action layer, so the name rides
/// a parallel registry set at request time).
static IRQ_LINE_NAMES: [Spinlock<&'static str>; 16] = [const { Spinlock::new("") }; 16];

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
const LINE_NAME_FALLBACK: [&str; 16] = [
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
/// tracepoints — the reference brackets every action where it executes
/// (kernel/irq/handle.c). The x86 line handlers are void, so the exit's
/// ret is "handled" — the handler ran. Both trace calls marshal only
/// behind armed() (one relaxed load when off; the name lookup too).
fn run_line_action(line: u8, h: u64) {
    if crate::trace::EVENT_IRQ_HANDLER_ENTRY.armed() {
        let name = irq_line_name(line);
        let name16 = if name.is_empty() {
            crate::trace::str_to_comm16(LINE_NAME_FALLBACK[line as usize])
        } else {
            crate::trace::str_to_comm16(name)
        };
        crate::trace::trace_irq_handler_entry(line as i32, &name16);
    }
    // SAFETY: nonzero values were stored from fn pointers by
    // request_irq_line_named.
    let f: fn() = unsafe { core::mem::transmute(h) };
    f();
    if crate::trace::EVENT_IRQ_HANDLER_EXIT.armed() {
        crate::trace::trace_irq_handler_exit(line as i32, 1);
    }
}

/// The per-PIC-line service threads ("irq/<line>-<name>").
static INTX_IRQ_THREADS: [ThreadedIrq; 16] = [const { ThreadedIrq::new() }; 16];

/// The virtio-blk MSI-X completion thread ("irq/129-blk", vector 0x81).
static BLK_MSIX_IRQ_THREAD: ThreadedIrq = ThreadedIrq::new();

/// Per-line INTx service thread body: run the line's handler (the ISR
/// read inside dispatches/ACKs the asserting virtio functions; the UART
/// handler drains the RX FIFO), then unmask. Unmask order matters: the
/// handler's device-side ACK drops the level assertion first, so the
/// unmask cannot release a storm (the reference's threaded level-IRQ
/// completion shape: thread runs action, then chip re-arms).
extern "C" fn intx_irq_thread_fn(arg: *mut core::ffi::c_void) -> i32 {
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
extern "C" fn blk_msix_irq_thread_fn(_arg: *mut core::ffi::c_void) -> i32 {
    BLK_MSIX_IRQ_THREAD.thread_loop(crate::drivers::virtio::msix_blk_isr)
}

/// Spawn (idempotent) the per-line thread at registration time. The
/// name must be &'static (kernel_thread holds it for the task lifetime):
/// "irq/<line>-<name>" is built and leaked, one tiny allocation per
/// threaded line, registration-time only. Returns whether a thread was
/// (already) spawned.
fn spawn_intx_irq_thread(line: u8, name: &'static str) -> bool {
    if INTX_IRQ_THREADS[line as usize].spawned() {
        return true; // skip the name leak on the idempotent path
    }
    let mut owned = alloc::vec::Vec::new();
    owned.extend_from_slice(b"irq/");
    owned.push(b'0' + (line / 10) as u8);
    owned.push(b'0' + (line % 10) as u8);
    owned.push(b'-');
    owned.extend_from_slice(if name.is_empty() {
        LINE_NAME_FALLBACK[line as usize].as_bytes()
    } else {
        name.as_bytes()
    });
    // SAFETY: "irq/" + digits + '-' + a registered &'static name is
    // valid UTF-8.
    let tname: &'static str = unsafe {
        core::str::from_utf8_unchecked(alloc::boxed::Box::leak(owned.into_boxed_slice()))
    };
    INTX_IRQ_THREADS[line as usize].spawn(
        tname,
        intx_irq_thread_fn,
        line as usize as *mut core::ffi::c_void,
    )
}

/// The MSI-X vector was programmed (probe-time marker for the deferred
/// boot pass — the spawn itself may have no-op'd pre-scheduler).
static BLK_MSIX_THREAD_WANTED: core::sync::atomic::AtomicBool =
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
    // rux:scribble (retired debug=scribble) inside crash_report itself).
    if !regs.is_null() {
        // SAFETY: diagnostic reads of the frame trap_exit just validated
        // (and rejected); the memory is the current task's stack.
        unsafe {
            crate::debug::scribble::crash_report(b"BAD-FRAME", &*regs);
        }
    }
    // Route the owner out like any fatal-user-fault: SIGSEGV, default
    // disposition kills. do_exit never returns.
    // huks-wedge lane (hw5): GROUP death — Linux's fixup_bad_iret
    // delivers a fatal SIGSEGV whose default action terminates the
    // whole process; a thread-only exit left the process alive with
    // the dead thread's userspace locks orphaned.
    crate::process::exit::do_exit_group(-(crate::signal::Signal::SIGSEGV as i32));
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
/// (arch-cold lane) the cross-stack foreign-entry report moved to the
/// arch service crate under forbid (one raw serial dump on a
/// detected corruption; the CPU parks right after — the park stays at
/// this call site). Historical-name wrapper dispatching the cold
/// ArchDiagHooks row.
fn scribble_foreign_entry(
    cpu: usize,
    vector: u64,
    regs: *mut PtRegs,
    tp: *mut crate::process::task::Task,
    frame: usize,
) {
    (crate::arch::arch_diag_hooks::arch_diag().scribble_foreign_entry)(cpu, vector, regs, tp, frame)
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
        use crate::debug::taskdump::{taskdump_dec, taskdump_raw_line};
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
    // SCRIBBLE2-GUARD (foreign-frame entry): a user-origin TRAP or IRQ
    // entry must land inside the CURRENT task's kernel stack — TSS.rsp0 /
    // PER_CPU.tss_rsp0 name exactly that stack.  A frame anywhere else is
    // the cross-stack engine caught in the act: this CPU would push frames
    // through the victim's live data (torn return addresses — the
    // byte-flip/#GP/#UD family).  Report the full ownership picture on raw
    // serial and park THIS CPU (the others keep their state for their own
    // reports).
    //
    // (gap-net) SYSCALL entries skip the walk: the entry stub validated
    // the GS base against the PerCpu magic BEFORE reading rsp0, and the
    // context switch validated the stack top it published into rsp0 (see
    // x86_64/context.rs) — a syscall frame is at tss_rsp0-168 by
    // construction, so this guard's content for syscalls (3 dependent
    // loads on the hottest path in the kernel) is already established by
    // those two checks.
    if cpu_id < crate::config::MAX_CPUS && vector != SYSCALL_VECTOR {
        let user_entry = unsafe { ((*regs).cs & 3) != 0 };
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
    // The frame edge: ONE audited reference formation per entry
    // (INV-XE-FRAME-1/2 in unsafe_edges.rs). Everything below is safe
    // Rust over the typed handle.
    let mut frame = unsafe { SavedFrame::<'static>::from_trap(regs) };

    // Save and swap PtRegs pointer. Nesting save/restore mirrors
    // the riscv64 twin: a timer IRQ landing inside a deeper trap
    // must not clobber the outer pointer. SYSCALL entries SKIP the
    // pair (three atomics on the hottest path in the kernel): fork
    // and exec derive the frame from task.pt_regs() (the canonical
    // outermost user frame at rsp0-168), so the slot has no
    // production reader for syscall frames — only the diagnostic
    // readers (taskdump, wakewatch cross-CPU rip), which are served
    // by the IRQ/exception stores and already document their races.
    {
        let syscall_vec = vector == SYSCALL_VECTOR;
        let prev_pt_regs = if syscall_vec {
            0
        } else {
            CURRENT_PT_REGS[cpu_id].load(core::sync::atomic::Ordering::Relaxed)
        };
        if !syscall_vec {
            CURRENT_PT_REGS[cpu_id].store(regs as u64, core::sync::atomic::Ordering::Relaxed);
        }

        // Scoped dispatch on the frame (regs_ref: the live &mut PtRegs).
        let regs_ref = &mut *frame;

        // OH bring-up: kernel-mode IPI/timer entries (0xE0-0xE5) are
        // routine events — printing every one flooded the emulated UART
        // (85% of a 400MB serial log) and slowed the whole boot enough
        // to cost samgr the hdf_devmgr race. The remaining per-trap
        // line (syscall entries are the overwhelming majority) is
        // likewise UART-flooding: it now needs rux:steptrace (retired debug=steptrace).
        let routine_ipi = !regs_ref.user_mode()
            && (0xE0..=0xE5).contains(&vector);
        if !routine_ipi
            && crate::trace::EVENT_STEPTRACE.armed()
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
            // rcu_user_exit shape (context tracking): a syscall ENTRY
            // from user mode proves the CPU just spent the whole
            // user-mode stretch outside any RCU read-side critical
            // section — report it as a quiescent state. This is the
            // user-busy CPU's report source the exit-tail report alone
            // misses when a peer's storm pins the console mid-write
            // (the bw_file_rd livelock: PID 1 blocked in write(1)
            // never returns to user, so the EXIT report never fires;
            // its next syscall ENTRY still does). One gated store,
            // same fast-path shape as the exit report.
            crate::sync::rcu::rcu_note_context_switch();
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
                    // HOTPATH-PRINT: unreachable per-hit — no NMI source
                    // exists on this board; one line per stray NMI.
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
                    // SP-5 r6 freeze ring: the 0xe0 service heartbeat
                    // entry, at entry before anything else (the re-arm
                    // lands as its own TIMER_OP_ONESHOT entry later in
                    // the tick handling).
                    crate::drivers::intc::apic::timer_event_record(
                        cpu_id,
                        crate::drivers::intc::apic::TIMER_OP_SERVICE,
                        0,
                        0,
                    );
                    crate::interrupt::preempt::TICK_ENTRY_PC[cpu_id].store(
                        crate::sched::current()
                            .map(|t| t.preempt_count())
                            .unwrap_or(0) as u32,
                        core::sync::atomic::Ordering::Relaxed,
                    );
                    // 118th monopolist capture: count consecutive ticks
                    // whose current task carries preempt_count >= 2 (a
                    // REAL leaked base pair — one base is any in-kernel
                    // spin section; two held across >= 50 ticks = 0.5 s is
                    // the unpreemptible incumbent shape). Diagnostic print
                    // only — no state is rewritten (the 29c2acba/393c04cf8
                    // ban is on wake-path heals and time-based slot
                    // CONVICTIONS, not on naming). One-shot per boot.
                    {
                        static MONO_TICKS: [core::sync::atomic::AtomicU32; 8] =
                            [const { core::sync::atomic::AtomicU32::new(0) }; 8];
                        static MONO_PID: [core::sync::atomic::AtomicU32; 8] =
                            [const { core::sync::atomic::AtomicU32::new(0) }; 8];
                        static MONO_DONE: core::sync::atomic::AtomicBool =
                            core::sync::atomic::AtomicBool::new(false);
                        let ci = (cpu_id & 7) as usize;
                        if let Some(t) = crate::sched::current() {
                            // SAFETY: read-only diagnostic access to the
                            // interrupted current task.
                            let (pc, pid) = unsafe { ((*t).preempt_count(), (*t).pid()) };
                            if pc >= 2 {
                                let prev_pid =
                                    MONO_PID[ci].load(core::sync::atomic::Ordering::Relaxed);
                                if prev_pid != pid as u32 {
                                    MONO_PID[ci].store(pid as u32, core::sync::atomic::Ordering::Relaxed);
                                    MONO_TICKS[ci].store(1, core::sync::atomic::Ordering::Relaxed);
                                } else {
                                    let n = MONO_TICKS[ci]
                                        .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                                    if n == 50
                                        && !MONO_DONE.swap(true, core::sync::atomic::Ordering::AcqRel)
                                    {
                                        // SAFETY: read-only comm access.
                                        let cb = unsafe { (*t).comm() };
                                        let cl = cb.iter().position(|&b| b == 0).unwrap_or(16);
                                        let comm =
                                            core::str::from_utf8(&cb[..cl]).unwrap_or("?");
                                        // HOTPATH-PRINT: budget-gated —
                                        // one-shot at 50 consecutive
                                        // monopolist ticks (MONO_TICKS
                                        // latch above).
                                        crate::pr_warn!(
                                            "MONOPOLIST-TICK pid={} ({}) preempt={} cpu={} last_rip={:#x} — remote bt follows",
                                            pid,
                                            comm,
                                            pc,
                                            cpu_id,
                                            regs_ref.rip
                                        );
                                        let _ = crate::arch::ipi::remote_backtrace(
                                            cpu_id as usize,
                                            "monopolist-tick",
                                        );
                                    }
                                }
                            } else {
                                MONO_TICKS[ci].store(0, core::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    }
                    // leg-33B: the interrupted RIP rides the same slot —
                    // the strand census's incumbent attribution
                    // (TICK_ENTRY_RIP).
                    crate::interrupt::preempt::TICK_ENTRY_RIP[cpu_id].store(
                        regs_ref.rip,
                        core::sync::atomic::Ordering::Relaxed,
                    );
                    // runq-strand lane: mode + identity + freshness for
                    // that RIP — the monopolist census needs to know
                    // whether the spin is USER or KERNEL mode and which
                    // incumbent the stamp belongs to.
                    crate::interrupt::preempt::TICK_ENTRY_CS[cpu_id].store(
                        regs_ref.cs as u64,
                        core::sync::atomic::Ordering::Relaxed,
                    );
                    crate::interrupt::preempt::TICK_ENTRY_PID[cpu_id].store(
                        crate::sched::current()
                            .map(|t| t.pid())
                            .unwrap_or(0),
                        core::sync::atomic::Ordering::Relaxed,
                    );
                    crate::interrupt::preempt::TICK_ENTRY_JIF[cpu_id].store(
                        crate::timer::get_jiffies(),
                        core::sync::atomic::Ordering::Relaxed,
                    );
                    // 118th param-stall conviction dump (one-shot once
                    // jiffies >= 30 000 = 300 s, rux:schedtrace (retired debug=schedtrace)-gated): the
                    // OH ladder stall reduces to init handling zero
                    // param-set messages — this dumps init's fd inventory +
                    // socket queue depths + epoll interest sets + the trace
                    // tail, naming which hypothesis owns it (ET edge loss /
                    // always-ready fd / empty-queue sender wedge). Diagnostic
                    // prints only; one per boot.
                    {
                        // Arm the LASTSYS latch from the first tick on
                        // hunt boots (see dispatch.rs lastsys_latch_arm).
                        crate::syscall::dispatch::lastsys_latch_arm();
                        // 118th PID-1 WATCH: the ladder break is init's
                        // main thread parked in a never-returning
                        // epoll_pwait(5,-1) while a sibling thread iterates
                        // the SAME epoll instance (721 scans) — the wake
                        // reaches one waiter and not the other. Put pid 1
                        // on the wakewatch from tick one so its ring names
                        // which stage (wake/link/pick) eats its events.
                        // 119th conviction fix: the old guard required
                        // current().is_none(), but a timer tick interrupts
                        // a live task — current() is Some — so the add
                        // NEVER RAN and the first capture's empty ring was
                        // an artifact of the guard, not evidence. Add
                        // unconditionally (once); ww_watch_add is
                        // idempotent anyway.
                        static WW_P1_DONE: core::sync::atomic::AtomicBool =
                            core::sync::atomic::AtomicBool::new(false);
                        if !WW_P1_DONE.load(core::sync::atomic::Ordering::Relaxed) {
                            // SAFETY: read-only lookup in the pid hash.
                            let t = unsafe { crate::sched::find_task_by_pid(1) };
                            if !t.is_null() {
                                crate::sched::sched::ww_watch_add(t);
                                WW_P1_DONE.store(true, core::sync::atomic::Ordering::Release);
                                // HOTPATH-PRINT: timer-tick one-shot/budgeted above
                                crate::pr_warn!(
                                    "WW-P1-WATCH-ADD task={:#x} landed at jiffies={}",
                                    t as usize,
                                    crate::timer::get_jiffies()
                                );
                            }
                        }
                        static PS_DONE: core::sync::atomic::AtomicBool =
                            core::sync::atomic::AtomicBool::new(false);
                        static PS_DONE2: core::sync::atomic::AtomicBool =
                            core::sync::atomic::AtomicBool::new(false);
                        if !PS_DONE2.load(core::sync::atomic::Ordering::Relaxed)
                            && crate::timer::get_jiffies() >= 24_000
                            && crate::trace::EVENT_SCHEDTRACE.armed()
                            && !PS_DONE2.swap(true, core::sync::atomic::Ordering::AcqRel)
                        {
                            // HOTPATH-PRINT: one-shot at 240s
                            // (PS_DONE2 latch above).
                            crate::pr_warn!("=== SECOND DUMP (240s trend) ===");
                            crate::syscall::support::param_stall_dump();
                        }
                        if !PS_DONE.load(core::sync::atomic::Ordering::Relaxed)
                            && crate::timer::get_jiffies() >= 18_000
                            && crate::trace::EVENT_SCHEDTRACE.armed()
                            && !PS_DONE.swap(true, core::sync::atomic::Ordering::AcqRel)
                        {
                            crate::syscall::support::param_stall_dump();
                        }
                    }
                    crate::timer::input_debug_tick();
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
                    crate::arch::ipi::handle_ipi_vector(v, regs_ref);
                    crate::interrupt::preempt::irq_exit();
                    irq_exit_maybe_preempt_kernel(regs_ref);
                    // tick_nohz_irq_exit: the kick (a dep 0->1 transition)
                    // re-evaluates the tick stop decision here.
                    crate::timer::nohz::tick_nohz_irq_exit();
                }

                // virtio-blk MSI-X completion vector (DIO-CONC, x86_64):
                // the posted-MSI delivery replaces the PIC-INTx + ISR-read
                // + Block-softirq chain. EOI first, same discipline as the
                // timer and IPI arms: the wake below can switch tasks.
                //
                // RT-plan R5: under forced threading (irqthread=1) the
                // walker no longer runs inline — the top half only wakes
                // the per-vector SCHED_FIFO 50 kthread ("irq/129-blk"),
                // which runs msix_blk_isr preemptibly (the reference's
                // threadirqs shape; mainline keeps the inline hardirq
                // walker, which is what the else-branch preserves). MSI
                // is edge-delivered: a completion arriving while the
                // thread runs posts a fresh vector and re-pends; the
                // pending flag coalesces and the walker drains the whole
                // used ring, so nothing is lost.
                v if v == crate::drivers::virtio::MSIX_BLK_VECTOR as u64 => {
                    crate::interrupt::preempt::irq_enter();
                    crate::drivers::intc::apic::eoi();
                    if !crate::interrupt::irqdesc::force_threaded()
                        || !BLK_MSIX_IRQ_THREAD.wake()
                    {
                        // Unthreaded, or the spawn race (vector live
                        // before the thread): run the walker inline.
                        crate::drivers::virtio::msix_blk_isr();
                    }
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

        // Restore previous PtRegs pointer (handles nested interrupt
        // case; syscalls never stored, so they never restore)
        if !syscall_vec {
            CURRENT_PT_REGS[cpu_id].store(prev_pt_regs, core::sync::atomic::Ordering::Relaxed);
        }
    }

    // Leak-origin tripwire (rux:leakhunt (retired debug=leakhunt)): this frame returns to
    // userspace, so the task's preempt_count must be exactly 0 (no
    // in-flight IRQ, every syscall-path guard dropped). A nonzero count
    // here is a leaked PREEMPT_OFFSET caught at the earliest possible
    // moment; the probe's per-task acquire-site shadow names the guard
    // construction that never balanced. Disabled-path cost: one relaxed
    // load. (Coroutine note: after an in-handler schedule() this tail
    // runs in the incoming task's own frozen frame with its own regs, so
    // the check validates whichever task actually returns to user.)
    if frame.user_mode() {
        // Return-to-user quiescent state (rcu.rs; see the riscv64 twin).
        crate::sync::rcu::rcu_note_context_switch();
        crate::debug::leakorigin::check_user_return();
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
    let now = crate::timer::get_jiffies();
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
/// legacy PIC IRQ0 fallback) — faithful port of the riscv64 twin.
fn handle_timer_tick(regs: &mut PtRegs, cpu: usize) {
    // Increment interrupt counter for /proc/interrupts
    crate::fs::perf_ctr::timer_inc(cpu);

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
            let cur_ptr = match &cur {
                Some(t) => (&**t) as *const _ as usize,
                None => 0,
            };
            let (pid, pc) = match &cur {
                Some(t) => (t.pid() as u64, t.preempt_count() as u64),
                None => (0, 0),
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
            // stress-port round 2 (2026-11-02): the tag now carries the
            // adjudication the suffix only hinted at. An idle frame is
            // halted with IF=1 (cpu_idle_loop re-enables IRQs before
            // wfi) — the gap is halt-exit delivery latency, an artifact
            // by construction. A user frame runs IF=1 by definition —
            // the gap is vCPU steal. Only a kernel frame keeps the
            // IRQ-OFF-WINDOW tag, and it is still only a triage hint:
            // the syscost cli/sti zone accounting is the corroborating
            // measurement (round-1 stress boots printed 101-461 ms
            // idle/user gaps while every syscost REST site's longest
            // CLOSED segment was <= 1 ms).
            let msg: &[u8] = if regs.user_mode() {
                b"\nWW: IRQ-DELAY-STEAL cpu="
            } else if is_idle {
                b"\nWW: IRQ-DELAY-IDLE cpu="
            } else {
                b"\nWW: IRQ-OFF-WINDOW cpu="
            };
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
            // mmi-endgame (116th): symbolize the rip LIVE with the
            // kernel's own ksyms table — the 115th's hw10 boots left
            // init spinning at one rip for 300+s (recurring 0.6-9.9 s
            // IF=0 windows) and the address could only be decoded
            // offline against that build's exact ELF, which no longer
            // existed (build/.config drift). Read-only static-table
            // binary search — IRQ-safe. Name capped to keep the
            // direct-console line bounded.
            if let Some((name, base, _sz)) = crate::debug::ksyms::lookup(regs.rip) {
                let msg: &[u8] = b" sym=";
                for &b in msg {
                    putchar(b);
                }
                for &b in name.as_bytes().iter().take(96) {
                    putchar(b);
                }
                let msg: &[u8] = b"+0x";
                for &b in msg {
                    putchar(b);
                }
                ph(regs.rip - base);
            }
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
    crate::fs::perf_ctr::charge_tick(regs.user_mode());

    // 0b. CPU-time accounting: charge the tick to the interrupted task,
    // user or system time by the mode the timer hit (user tasks only).
    if let Some(task) = crate::sched::current() {
        // current()'s typed &mut handle; we only touch per-task atomic
        // counters (no teardown race on these fields).
        {
            if task.address_space().is_some() {
                use core::sync::atomic::Ordering::Relaxed;
                if regs.user_mode() {
                    task.utime_ticks.fetch_add(1, Relaxed);
                    crate::debug::syscost::tick_user();
                    // leg-40C perf rider: user-mode rip sampling for
                    // launcher-comm tasks (debug-gated inside).
                    utime_sample_rip(regs.rip, task.comm());
                    // perf events (PERF_TYPE_SOFTWARE sampling): stash the
                    // interrupted PC + frame pointer for the softirq
                    // charger (RBP feeds the user callchain walk).
                    // Lock-free.
                    crate::perf::note_tick_fp(
                        regs.rip,
                        crate::perf::MODE_USER,
                        task.pid(),
                        regs.rbp,
                        task.tgid(),
                    );
                } else {
                    task.stime_ticks.fetch_add(1, Relaxed);
                    // sc105 stime-burn attribution: sample WHERE the
                    // kernel-mode tick landed (4 KiB page bucket keyed by
                    // task comm, so one task's burn is separable from the
                    // boot crowd). KILLDUMP prints the table.
                    stime_sample_rip(regs.rip, task.comm());
                    if comm_has_launcher(task.comm()) {
                        STIME_LAUNCHER_TICKS.fetch_add(1, Relaxed);
                    }
                    crate::debug::syscost::tick_kern(regs.rip);
                    crate::perf::note_tick_fp(
                        regs.rip,
                        crate::perf::MODE_KERNEL,
                        task.pid(),
                        regs.rbp,
                        task.tgid(),
                    );
                }
            } else {
                crate::debug::syscost::tick_idle();
                crate::perf::note_tick(regs.rip, crate::perf::MODE_IDLE, 0);
            }
        }
    } else {
        crate::debug::syscost::tick_idle();
        crate::perf::note_tick(regs.rip, crate::perf::MODE_IDLE, 0);
    }

    // 1. Update jiffies (+ RT-2X item 4: the driver entry also expires
    // due high-resolution timers HERE, at the top of the IRQ — the
    // reference's hrtimer_interrupt shape; see
    // drivers/timer/x86_64.rs and timer::expire_hres_inline).
    let jiffies_advanced = crate::drivers::timer::timer_interrupt_handler();

    // RT-2X item 4, light exit: an hres-only fire (a precise deadline
    // between grid ticks — jiffies did not move) carries no scheduler
    // tick duty. The reference's tick_nohz machinery gives tickless
    // CPUs exactly this shape: hrtimer fires deliver their wakeups and
    // return, without running account_process_tick / scheduler_tick /
    // watchdogs. Before this branch, EVERY LAPIC fire on a precise-
    // sleep workload paid the full body below — the measured 30-70 us
    // class between the deadline fire and the sleeper's wake
    // (run/RT-2X.md, t_abs rows). The nohz re-arm still runs (this fire
    // may have been the armed next-event); the softirq Hrtimer raise
    // from the driver entry drains at irq_exit and hits its dedupe
    // fast-out (the inline expiry already drained the wheel).
    if jiffies_advanced == 0 {
        crate::timer::nohz::tick_nohz_irq_exit();
        return;
    }

    // 1.1 Re-arm the LAPIC timer BEFORE anything below can switch tasks
    // (Tick re-arm moved to the END of this handler as
    // tick_nohz_irq_exit() — the reference's tick_nohz machinery
    // re-evaluates the stop decision AFTER the tick work, at irq_exit.)

    // 1.2 sc109: 1 Hz drift discipline for the HPET-calibrated TSC
    // timekeeper (offset-only forward steps, clamped; see
    // x86_64::resync_tsc_against_hpet).
    {
        let j = crate::drivers::timer::x86_64::get_jiffies();
        if j % crate::config::KERNEL_HZ as u64 == 0 {
            crate::drivers::timer::x86_64::resync_tsc_against_hpet();
            // leg-40C perf rider: 1 Hz boundary rides the drift block;
            // self-throttled to 60 s inside.
            utime_ripsample_report();
        }
    }

    // 1.5 Scribble hunter (rux:scribble (retired debug=scribble)): GS-pairing tripwire + verify
    // quiescent tasks' tracked scheduling fields against their shadows +
    // scheduler-consistency scan (double-run seed detection).
    // Throttled inside; no-op unless the runtime switch is on.
    crate::debug::scribble::verify_gs();
    crate::debug::scribble::verify();
    crate::debug::scribble::verify_consistency();

    // 2. Scheduler tick
    crate::sched::scheduler_tick();

    // 3. Check for soft lockups
    crate::debug::softlockup::check();

    // 4. Reschedule if needed (the trap-exit asm path owns the user-mode
    //    preemption; this in-IRQ switch only fires when preemptible,
    //    which is never true inside irq_enter — kept for twin parity).
    if crate::sched::need_resched() && crate::interrupt::preempt::preemptible() {
        crate::sched::schedule();
    }

    // 5. tick_nohz_irq_exit (tick-sched.c): after the tick work, decide
    //    whether this CPU's periodic tick can stop (can_stop_full_tick)
    //    or must resume (a dependency appeared) — the re-arm that the
    //    pre-nohz set_next_trigger call above used to do.
    crate::timer::nohz::tick_nohz_irq_exit();
}

/// Handle timer interrupt on the legacy PIC IRQ0 vector (virtual-wire
/// fallback; the LAPIC timer vector routes to `handle_timer_tick`
/// directly with a LAPIC EOI).
fn handle_timer_interrupt(regs: &mut PtRegs, cpu: usize) {
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
fn handle_external_interrupt(vector: u64) {
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
fn handle_syscall(regs: &mut PtRegs) {
    // forkprof trace: first Rust code of a syscall (entry side). The
    // nr rides a second event (site 15, pid field = nr) so the timeline
    // names each child-side call.
    if crate::debug::forkprof::trace_on() {
        crate::debug::forkprof::trace(14, crate::sched::get_current_pid()); // T_SYSENTRY
        crate::debug::forkprof::trace(15, regs.rax as u32); // T_SYSNR
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
    if let Some(rv) = crate::syscall::seccomp::seccomp_syscall_gate(syscall_num, regs) {
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
            // HOTPATH-PRINT: budget-gated (8/boot) — SIGILL-probing
            // media/JIT workloads fault repeatedly; the signal delivery
            // carries the rest.
            static UD_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            if UD_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
            crate::pr_info!(
                "trap: #UD at rip={:#x} pid {} comm {} bytes {}",
                regs.rip,
                pid,
                comm,
                hex
            );
            }
        }
        if sigill_has_handler() && !sigsegv_blocked_current() {
            let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGILL as i32);
            crate::signal::force_unblock_current(crate::signal::Signal::SIGILL as i32);
            return;
        }
        let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGILL as i32);
        // huks-wedge lane (hw5): fault-path default deaths are GROUP
        // deaths (Linux: fatal default action → do_group_exit).
        crate::process::exit::do_exit_group(-(crate::signal::Signal::SIGILL as i32));
        // Do NOT advance rip — the task is ZOMBIE and will not resume
    } else {
        // A kernel-mode #UD is always a kernel bug; skipping it would
        // corrupt execution. The Oops path prints the report and
        // applies the policy (Tranche 5, the riscv64 twin's).
        #[cfg(feature = "x86_64")]
        crate::debug::scribble::crash_report(b"#UD", regs);
        crate::debug::oops::oops(
            crate::debug::oops::Kind::IllegalInstruction,
            &|w: &mut crate::debug::backtrace::ConsoleWriter| {
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
        // DEATH-ONLY (kallsyms conviction, 2026-11-22): the dumps moved
        // below the handler check — a HANDLED #GP (crash catcher,
        // deliberate probe) must not pay the full forensic decode; LTP's
        // kallsyms deliberately faults through a handler and the
        // per-fault dump turned it into a 90s-timeout SIGKILL.
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
        user_segv_forensics(regs, regs.rip);
        gp_regmem_dump(regs);
        gp_mmhistory_dump(regs);
        let _ = crate::signal::send_signal_with_info_directed(pid, gp_info);
        // huks-wedge lane (hw5): group death (Linux do_group_exit).
        crate::process::exit::do_exit_group(-(crate::signal::Signal::SIGSEGV as i32));
    } else {
        #[cfg(feature = "x86_64")]
        crate::debug::scribble::crash_report(b"#GP", regs);
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
            // huks-wedge lane (hw5): group death (Linux do_group_exit).
            crate::process::exit::do_exit_group(-(crate::signal::Signal::SIGFPE as i32));
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
            // huks-wedge lane (hw5): group death (Linux do_group_exit).
            crate::process::exit::do_exit_group(-(crate::signal::Signal::SIGBUS as i32));
        }
    } else {
        panic!("trap: #AC in kernel mode at rip={:#x}", regs.rip);
    }
}

/// #NM — FPU unavailable: the LAZY-FPU restore point.
///
/// context_switch sets CR0.TS on every switch-in and skips the incoming
/// task's xrstor; the task's first FP instruction lands here (the kernel
/// binary has no compiler-emitted FP instructions — the only xmm sites
/// are restore_fpu's own asm — so a user instruction is the only way to
/// observe TS). Clear TS and run the ordinary restore: the saved image
/// when the task ever used FP, the deterministic ABI-initial state
/// otherwise (fork/exec mark fpu images clean/valid).
fn handle_device_not_available() {
    if let Some(cur) = crate::sched::current() {
        // SAFETY: cur is the task now running on this CPU; clts claims
        // the live FPU for it, restore_fpu loads its image.
        unsafe {
            super::cpu::clts();
            (*cur).thread_mut().restore_fpu();
        }
        return;
    }
    // No current task (early boot / idle context) took an FP trap:
    // re-enable the FPU defensively instead of looping on #NM.
    // HOTPATH-PRINT: rare self-heal arm — fires only when the #NM
    // handler finds no current task; re-runs fpu_init and returns.
    crate::pr_warn!("trap: #NM with no current task — re-running fpu_init");
    // SAFETY: fpu_init only flips CR0/CR4 feature bits and zeroes the
    // live FPU state; the caller was not using it (#NM proves that).
    unsafe { super::thread::fpu_init() };
}

/// #DB — hardware single-step (TF) or debug-register trap.
fn handle_debug(regs: &mut PtRegs) {
    const TF: u64 = 1 << 8;
    const RF: u64 = 1 << 16;
    if regs.user_mode() {
        // sc57: fuxwatch single-step retirement — the store completed;
        // re-protect the watched page (W off again for the next write)
        // and continue silently.
        // huks-wedge lane (hw2): re-protect the page the STEP recorded
        // (the store's own page), and only while the watch still owns it
        // — after a slot move the retired page must stay writable.
        let tid = crate::sched::get_current_pid() as u32;
        if let Some(page) = crate::sync::futex::fuxwatch_take_step(tid) {
            regs.rflags &= !(TF | RF);
            if crate::sync::futex::fuxwatch_page_watched(page) {
                if let Some(tk) = crate::sched::current() {
                    // SAFETY: current task accessor.
                    if let Some(aspace) = unsafe { (*tk).address_space() } {
                        let root = aspace.root_ppn();
                        let g = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock();
                        // SAFETY: current mm's live root; one page, R.
                        unsafe {
                            crate::arch::mm::mm_ops::mprotect_pte_range(
                                root,
                                page,
                                1,
                                0x1,
                            );
                        }
                        drop(g);
                        super::mm::asid::flush_tlb_page(page);
                    }
                }
            }
            return;
        }
        // Stop stepping before delivering anything, or iretq/sysret
        // would re-trap on the first handler/user instruction.
        regs.rflags &= !TF;
        if let Some(task) = crate::sched::current() {
            if (*task).tracer_pid() != 0 {
                // SAFETY: task is the current task in its trap path.
                unsafe {
                    crate::process::ptrace_core::ptrace_stop(
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
        // Linux do_debug: a user #DB without a tracer is a CATCHABLE
        // SIGTRAP (force_sig_fault semantics). The old unconditional
        // do_exit killed every task that set TF with a handler
        // installed (gdb-less tracers, coverage tools, the tfprobe
        // A/B) — the reference only kills on the default disposition.
        // Handler-first routing like the #UD/#GP arms; a blocked
        // SIGTRAP (trap inside its own handler) is forced to the
        // default action, mirroring the SIGSEGV discipline.
        let pid = crate::process::current_pid();
        let _ = crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGTRAP as i32);
        if !sigtrap_has_handler() || sigtrap_blocked_current() {
            crate::process::exit::do_exit(-(crate::signal::Signal::SIGTRAP as i32));
        } else {
            crate::signal::force_unblock_current(crate::signal::Signal::SIGTRAP as i32);
        }
    } else {
        // Kernel-mode #DB with TF set: clear TF in the saved frame and
        // resume (Linux do_debug discipline for kernel steps).
        regs.rflags &= !TF;
    }
}

/// SIGTRAP (5) disposition check for the #DB path (step tracers and
/// coverage tools install one; the default disposition kills).
fn sigtrap_has_handler() -> bool {
    crate::sched::current()
        .and_then(|t| (*t).signal.as_ref().map(|s| s.get_action(5)))
        .map(|a| a.is_some_and(|a| a.has_handler()))
        .unwrap_or(false)
}

/// True when the CURRENT task blocks SIGTRAP (bit 3) — the thread is
/// inside a SIGTRAP handler without SA_NODEFER (a trap-in-handler
/// loop would otherwise refault forever).
fn sigtrap_blocked_current() -> bool {
    crate::sched::current()
        .map(|t| t.sigmask & (1u64 << 3) != 0)
        .unwrap_or(false)
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
                let restored = crate::process::ptrace_core::write_target_word(task, step_addr, saved);
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
                        crate::process::ptrace_core::ptrace_stop(
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
                    crate::process::ptrace_core::ptrace_stop(
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

/// #GP register-memory dump (hp88 hunt): two u64 words at every
/// canonical-user pointer register, read through the checked walk. The
/// mallocng get_meta #GP dereferences a metadata pointer read from the
/// heap — this shows what the surrounding heap words contain (perf-
/// record-shaped bytes would name the corrupting writer directly).
// HOTPATH-PRINT (whole fn): fatal-path forensics — called once from
/// (arch-cold lane) the #GP register-set memory/PTE dump, the tgid
/// mm-syscall history dump, the fatal user SIGSEGV death report (with
/// its nested mm-ring and page-walk readers) and the SEGV-history ring
/// moved to the arch service crate under forbid; these
/// historical-name wrappers dispatch the cold ArchDiagHooks rows (the
/// death report is one bounded dump per user-fault death — the process
/// exits immediately after).
fn gp_regmem_dump(regs: &PtRegs) {
    (crate::arch::arch_diag_hooks::arch_diag().gp_regmem_dump)(regs)
}

fn gp_mmhistory_dump(regs: &PtRegs) {
    (crate::arch::arch_diag_hooks::arch_diag().gp_mmhistory_dump)(regs)
}

/// Age (in jiffies) of this tid's most recent delivered SIGSEGV, plus
/// that fault's cr2, if the tid ever took one. (arch-cold lane: the
/// ring + this reader live in arch; the wrapper keeps the
/// historical path for do_exit's holder-exit print.)
pub fn segv_hist_age(tid: u32) -> Option<(u64, u64)> {
    (crate::arch::arch_diag_hooks::arch_diag().segv_hist_age)(tid)
}

fn user_segv_forensics(regs: &PtRegs, fault_addr: u64) {
    (crate::arch::arch_diag_hooks::arch_diag().user_segv_forensics)(regs, fault_addr)
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

    if crate::trace::EVENT_STEPTRACE.armed() {
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

    // sc57 (57th): fuxwatch write watchpoint — a user WRITE fault on
    // the watched page logs the writer, restores W for the retried
    // instruction, and single-steps it via TF; the #DB hook re-clears
    // W so the next write traps again. Zero cost when the param is
    // absent (one relaxed load).
    // 84th: KERNEL-mode WRITE faults on the watched page (a syscall's
    // put_user — clock_gettime's timespec out-param lives on the same
    // arena control page as the watched futex word) are handled here
    // too: restore W, name the kernel writer, retry. Before, they fell
    // to the kernel-fill path, hit the W-cleared PTE
    // (PermissionDenied), surfaced as EFAULT — libc++ turned that into
    // system_error and the daemons SIGABRT'd (ohw156 t=161.5 installs,
    // ohw157/158 t=33.9 telecom — the instrument was killing its own
    // subject). No TF single-step for kernel writers.
    // huks-wedge lane (hw2): the over-budget check moved INSIDE — an
    // over-budget skip left the W-cleared PTE with no handler and the
    // write fell to do_page_fault → SEGV_ACCERR. Every fault on a page
    // WE armed gets W restored and retried; only the log/step respects
    // the budget (fuxwatch_log_write retires the watch at the trip).
    if access & crate::arch::mm::page_fault::FaultFlags::WRITE != 0
        && crate::sync::futex::fuxwatch_page_matches(fault_addr)
    {
        let is_user = access & crate::arch::mm::page_fault::FaultFlags::USER != 0;
        let tid = crate::sched::get_current_pid() as u32;
        let rip = regs.rip;
        // Handle only mms WE armed (a fork-COW write fault on this VA
        // in an un-armed mm falls through to the normal path).
        let handled = crate::sched::current().and_then(|tk| {
            // SAFETY: current task accessor.
            let aspace = unsafe { (*tk).address_space() }?;
            let root = aspace.root_ppn();
            if !crate::sync::futex::fuxwatch_mm_armed(root) {
                // neverword lane (nw1 conviction, safety net): an mm the
                // arm round once covered but the ledger no longer lists
                // (slot-latch reopen, fork-inherit of a W-cleared PTE)
                // used to fall to do_page_fault -> present-RO PTE ->
                // SEGV_ACCERR and group-kill its process (nw1 samgr,
                // hg1 hilogd). A write fault on a WATCHED page with a
                // present+user+W-clear PTE in a writable mapping is a
                // leak of OUR watch, not a permission denial: restore W,
                // retry unlogged, warn once.
                let g = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock();
                // SAFETY: read-only diagnostic walk of the faulting mm.
                let leaked = unsafe {
                    crate::arch::mm::mm_ops::PageTableWalker::walk(
                        root,
                        fault_addr & !0xfff,
                    )
                }
                .map(|(_, bits)| bits & 0x1 != 0 && bits & 0x4 != 0 && bits & 0x2 == 0)
                .unwrap_or(false);
                if leaked {
                    // SAFETY: current mm's live root; one page, RW.
                    unsafe {
                        crate::arch::mm::mm_ops::mprotect_pte_range(
                            root,
                            fault_addr & !0xfff,
                            1,
                            0x3,
                        );
                    }
                    drop(g);
                    super::mm::asid::flush_tlb_page(fault_addr);
                    static LEAK_N: core::sync::atomic::AtomicUsize =
                        core::sync::atomic::AtomicUsize::new(0);
                    // HOTPATH-PRINT: budget-gated (20/boot) — the futex
                    // watch fires per access to the watched word.
                    if LEAK_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 20 {
                        crate::pr_warn!(
                            "FUXWATCH-LEAK-FIX tid={} rip={:#x} addr={:#x} W restored, unlogged",
                            tid,
                            rip,
                            fault_addr
                        );
                    }
                    return Some(());
                }
                drop(g);
                return None;
            }
            let g = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock();
            // SAFETY: current mm's live root; one page, RW.
            unsafe {
                crate::arch::mm::mm_ops::mprotect_pte_range(
                    root,
                    fault_addr & !0xfff,
                    1,
                    0x3,
                );
            }
            drop(g);
            super::mm::asid::flush_tlb_page(fault_addr);
            // hw2: exact/old_word describe the WORD THAT FAULTED (either
            // slot) — slot-2 writes used to log with slot 1's exact=false
            // and slot 1's old value.
            let w1 = crate::sync::futex::fuxwatch_addr();
            let w2 = crate::sync::futex::fuxwatch2_addr();
            let exact = (w1 != 0 && (fault_addr & !0x3) == (w1 & !0x3))
                || (w2 != 0 && (fault_addr & !0x3) == (w2 & !0x3));
            let old_word = crate::sync::futex::fuxwatch_read_word_at(root, fault_addr & !0x3);
            if !is_user {
                // Kernel writer (put_user on the watched page): named
                // via a budgeted line, retried with W present.
                static KFW_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                // HOTPATH-PRINT: budget-gated (40/boot) — see LEAK-FIX
                // above.
                if KFW_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
                    crate::pr_warn!(
                        "FUXWATCH-KWRITE tid={} rip={:#x} addr={:#x} exact={} old={:#x}",
                        tid,
                        rip,
                        fault_addr,
                        exact,
                        old_word
                    );
                }
            } else if crate::sync::futex::fuxwatch_log_write(tid, rip, exact, old_word, fault_addr) {
                // Step the writer: TF traps right after the store;
                // RF masks fault-class re-entry for this insn.
                regs.rflags |= (1 << 8) | (1 << 16);
            }
            Some(())
        });
        if handled.is_some() {
            return; // retry the instruction
        }
    }

    let result = do_page_fault(regs, access);

    match result {
        MmFaultResult::Handled | MmFaultResult::Fixed => {
            // Page handled, re-execute instruction
        }
        MmFaultResult::Segfault => {
            // HOTPATH-PRINT: budget-gated (8/boot) — one line per user
            // SIGSEGV fault otherwise; the per-task death report below
            // carries the full forensics.
            static SEGV_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            // Defect-C: the FIRST refusal per task (the static gets
            // overwritten by successive faults; later prints mislead).
            let first_for_task = crate::debug::forensics_edges::mark_first_refusal(
                crate::sched::get_current_pid() as u64,
            );
            // HOTPATH-PRINT: budget-gated — 8 total (SEGV_N) + the first
            // refusal per task (defect-C's one-per-pid); the fault path is
            // per-task-fatal, never a loop rate.
            if SEGV_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 || first_for_task {
            crate::pr_err!(
                "pagefault: Segfault at {:#x}, rip={:#x}, rsp={:#x}, pid={}, mode={}, fill_site={}",
                fault_addr,
                regs.rip,
                regs.rsp,
                crate::sched::get_current_pid(),
                if regs.kernel_mode() { "kernel" } else { "user" },
                crate::arch::mm::page_fault::FILL_REFUSAL_SITE.load(core::sync::atomic::Ordering::Relaxed)
            );
            }
            if regs.user_mode() {
                // Linux semantics: route user-mode faults through signal
                // delivery (force_sig_fault) — an installed SIGSEGV
                // handler must run (LTP mmap05); default kills here.
                // A blocked SIGSEGV (fault inside its own handler) is
                // forced to the default action instead of pended — see
                // sigsegv_blocked_current.
                // si_code/si_addr parity (arena-mmap lane 2026-11-14):
                // force_sig_fault(SEGV, SEGV_MAPERR, addr) — the siginfo
                // must carry the faulting address, or any handler that
                // demand-maps the faulting page (FFRT arenas, GC guards)
                // installs the WRONG page and the instruction re-faults
                // forever (the 0x7ff1 OH fault-loop family).
                let pid = crate::process::current_pid();
                let info = crate::signal::SigInfo::fault(
                    crate::signal::Signal::SIGSEGV as i32,
                    crate::signal::si_code::SEGV_MAPERR,
                    fault_addr as u64,
                );
                if sigsegv_has_handler() && !sigsegv_blocked_current() {
                    let _ = crate::signal::send_signal_with_info_directed(pid, info);
                    return;
                }
                // DEATH-ONLY forensics (kallsyms conviction, 2026-11-22):
                // the dump fires ONLY on the default disposition. Fired
                // before the handler check it taxed every HANDLED fault —
                // LTP's kallsyms probes 37k kernel addresses through a
                // SIGSEGV+siglongjmp handler and drowned in dumps until
                // the harness timeout SIGKILLed it (FAIL->CRASH(137)).
                user_segv_forensics(regs, fault_addr);
                let _ = crate::signal::send_signal_with_info_directed(pid, info);
                // huks-wedge lane (hw5): GROUP death — a synchronous
                // SIGSEGV with default disposition terminates the whole
                // process under Linux (do_group_exit from get_signal);
                // the old thread-only do_exit left the process alive
                // with the dead thread's userspace locks orphaned —
                // hw5w1 t=5.44: huks_service tid 486 died alone of
                // SIGSEGV (grpexit=false), the process survived, FOUR
                // musl __lock words froze, every client parked forever
                // (softbus/accountmgr/foundation/distributeddata/mmi).
                crate::process::exit::do_exit_group(-(crate::signal::Signal::SIGSEGV as i32));
            } else {
                #[cfg(feature = "x86_64")]
                crate::debug::scribble::crash_report(b"#PF-segv", regs);
                // The riscv64 mm layer returns KernelPanic for kernel
                // faults; until the x86 twin grows that, do NOT silently
                // iretq back into the faulting rip (infinite fault loop).
                // The Oops path prints the report and applies the policy
                // (panic in interrupt context / oops=panic / idle, task
                // death otherwise — Tranche 5).
                crate::debug::oops::oops(
                    crate::debug::oops::Kind::PageFault { addr: fault_addr },
                    &|w: &mut crate::debug::backtrace::ConsoleWriter| {
                        oops_show_regs(w, regs);
                    },
                );
            }
        }
        MmFaultResult::PermissionDenied => {
            // HOTPATH-PRINT: budget-gated (8/boot) — W^X/probing workloads hit
    // this per store; the SIGSEGV delivery (not this line) is the
    // observable event.
    static PDENY_N: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    if PDENY_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
    crate::pr_err!("pagefault: Permission denied at {:#x}, rip={:#x}, pid={}",
                fault_addr, regs.rip, crate::sched::get_current_pid());
    }
            if regs.user_mode() {
                // Handler-first routing like Segfault (PROT_NONE access);
                // likewise force-default a blocked SIGSEGV. si_code is
                // SEGV_ACCERR (permission denied, the PROT_NONE class)
                // with si_addr = cr2 — FFRT's arena handler maps exactly
                // this page (arena-mmap lane 2026-11-14).
                let pid = crate::process::current_pid();
                let info = crate::signal::SigInfo::fault(
                    crate::signal::Signal::SIGSEGV as i32,
                    crate::signal::si_code::SEGV_ACCERR,
                    fault_addr as u64,
                );
                if sigsegv_has_handler() && !sigsegv_blocked_current() {
                    let _ = crate::signal::send_signal_with_info_directed(pid, info);
                    return;
                }
                // DEATH-ONLY forensics (kallsyms conviction, 2026-11-22):
                // see the SEGV_MAPERR arm — handled faults skip the dump.
                user_segv_forensics(regs, fault_addr);
                let _ = crate::signal::send_signal_with_info_directed(pid, info);
                // huks-wedge lane (hw5): group death (Linux do_group_exit)
                // — see the SEGV_MAPERR arm above for the full rationale.
                crate::process::exit::do_exit_group(-(crate::signal::Signal::SIGSEGV as i32));
            } else {
                #[cfg(feature = "x86_64")]
                crate::debug::scribble::crash_report(b"#PF-perm", regs);
                // Oops path (Tranche 5): report + policy, like the
                // Segfault arm above.
                crate::debug::oops::oops(
                    crate::debug::oops::Kind::PageFault { addr: fault_addr },
                    &|w: &mut crate::debug::backtrace::ConsoleWriter| {
                        oops_show_regs(w, regs);
                    },
                );
            }
        }
        MmFaultResult::BusError => {
            // HOTPATH-PRINT: budget-gated (8/boot) — apps probing past EOF hit
    // this per access.
    static EOF_N: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    if EOF_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
    crate::pr_err!("pagefault: Bus error (past EOF) at {:#x}", fault_addr);
    }
            if regs.user_mode() {
                // Handler-first routing like Segfault: LTP mmap13 installs
                // a SIGBUS handler and must catch it; the default action
                // kills with SIGBUS. force_unblock (the riscv64 twin):
                // a blocked SIGBUS would otherwise stay pended while the
                // task returns to the faulting rip and refaults forever —
                // the same livelock the SIGSEGV arm guards against.
                // si_code/si_addr parity (arena-mmap lane): BUS_ADRERR +
                // fault_addr, Linux force_sig_bus shape.
                let pid = crate::process::current_pid();
                let _ = crate::signal::send_signal_with_info_directed(
                    pid,
                    crate::signal::SigInfo::fault(
                        crate::signal::Signal::SIGBUS as i32,
                        crate::signal::si_code::BUS_ADRERR,
                        fault_addr as u64,
                    ),
                );
                if !sigbus_has_handler() {
                    // huks-wedge lane (hw5): group death (Linux do_group_exit).
                    crate::process::exit::do_exit_group(-(crate::signal::Signal::SIGBUS as i32));
                } else {
                    crate::signal::force_unblock_current(crate::signal::Signal::SIGBUS as i32);
                }
            } else {
                #[cfg(feature = "x86_64")]
                crate::debug::scribble::crash_report(b"#PF-bus", regs);
                // Oops path (Tranche 5): report + policy, like the
                // Segfault arm above.
                crate::debug::oops::oops(
                    crate::debug::oops::Kind::PageFault { addr: fault_addr },
                    &|w: &mut crate::debug::backtrace::ConsoleWriter| {
                        oops_show_regs(w, regs);
                    },
                );
            }
        }
        MmFaultResult::OutOfMemory => {
            // HOTPATH-PRINT: budget-gated (8/boot) — a looping OOM app fires
    // this per fault; the kill itself is the event.
    static OOM_N: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    if OOM_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
    crate::pr_err!("pagefault: Out of memory at {:#x}", fault_addr);
    }
            crate::mm::page_alloc::oom_forensic_dump("pagefault OOM");
            if regs.user_mode() {
                // fake-OOM family: Linux pagefault_out_of_memory parity —
                // give the global OOM killer (badness-based victim
                // selection, rate-limited) the chance to free the real
                // hog before this task dies. Retry on true, die on false
                // (struck out or nothing killable).
                if !crate::mm::mm_hooks::fault_oom_kill_or_retry(
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
                // gw2 (oh-launch round 6, gdbstub on the live wedge): a
                // CPU arriving here parks in the eternal halt loop WITH
                // WHATEVER IT HELD — seven CPUs spun on the GRQ behind
                // one such parker. Release the stranded runqueue lock
                // (positive knowledge via the lock word's owner field —
                // terminal_release_grq_if_held reads held_by(cpu), the
                // single authority) BEFORE halting; the section it
                // protected is dead with this CPU either way.
                // uart_putc_raw: the old marker was putchar_no_lock —
                // SILENT on release builds, so these parks were
                // invisible (every "silent" permanent wedge).
                for &b in b"trap: KERNPANIC re-entered during dump; halting cpu=" {
                    crate::console::uart_putc_raw(b);
                }
                {
                    let c = crate::arch::cpu_id();
                    if c < 10 {
                        crate::console::uart_putc_raw(b'0' + c as u8);
                    } else {
                        crate::console::uart_putc_raw(b'A' + (c - 10) as u8);
                    }
                }
                crate::console::uart_putc_raw(b'\n');
                crate::sched::sched::terminal_release_grq_if_held();
                // R11 (the parked-CPU interrupt strand, fr7 conviction):
                // the halt loop must run with INTERRUPTS DISABLED. The old
                // shape parked with whatever IF the fault arrived under —
                // an IF=1 park wakes on the next timer tick, and the tick
                // handler then runs scheduler_tick ON TOP of the broken
                // dumper context: it acquires the GRQ (word=1, tracker
                // set, site=tick) and wedges/faults inside the section,
                // stranding the runqueue forever — fr7's crime scene: the
                // FIRST holder ever named (cpu4/pid356 inside
                // scheduler_tick) with TERM-GRQ-FORCE at ZERO fires (the
                // park-time tracker was clear; the acquire came from the
                // post-park interrupt). The reference shape is
                // panic_smp_self_stop: local_irq_disable before the
                // eternal halt — a dead CPU must never take interrupts
                // again.
                crate::arch::cpu::disable_irq();
                // SAFETY: hlt with IF=0 never wakes — the intentional
                // permanent park of a crashed CPU.
                loop {
                    unsafe { core::arch::asm!("hlt") };
                }
            }
            // SCRIBBLE2: park hook for gdb (walk the faulting CR3's page
            // tables live from the debugger).
            #[cfg(feature = "x86_64")]
            crate::debug::scribble::kernpanic_park(fault_addr, regs.rip, regs.rsp);
            // Raw serial print (printk may be wedged on a lock this CPU
            // holds), then halt — the twin's R9 discipline.
            // uart_putc_raw writes the serial port directly (no locks).
            // (x86-smprace: the old (0..64).step_by(4).rev() loops hit a
            // shift-overflow panic on their final iterator step; the
            // explicit while form below cannot.)
            fn kp_put_hex(v: u64) {
                let mut sh: i32 = 64;
                while sh > 0 {
                    sh -= 4;
                    let nb = ((v >> sh) & 0xF) as u8;
                    crate::console::uart_putc_raw(if nb < 10 {
                        b'0' + nb
                    } else {
                        b'a' + nb - 10
                    });
                }
            }
            for &b in b"trap: KERNPANIC pfault badaddr=0x" {
                crate::console::uart_putc_raw(b);
            }
            kp_put_hex(fault_addr);
            for &b in b" rip=0x" {
                crate::console::uart_putc_raw(b);
            }
            kp_put_hex(regs.rip);
            for &b in b" rsp=0x" {
                crate::console::uart_putc_raw(b);
            }
            kp_put_hex(regs.rsp);
            crate::console::uart_putc_raw(b'\n');
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
                        crate::console::uart_putc_raw(b);
                    }
                    break;
                }
                // SAFETY: bounded read of our own kernel stack (see the
                // R73 comment above).
                let v = unsafe { core::ptr::read_volatile(addr as *const u64) };
                for &b in b"kp-stk " {
                    crate::console::uart_putc_raw(b);
                }
                kp_put_hex(addr as u64);
                crate::console::uart_putc_raw(b':');
                crate::console::uart_putc_raw(b' ');
                kp_put_hex(v);
                crate::console::uart_putc_raw(b'\n');
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
                    crate::console::uart_putc_raw(b);
                }
                kp_put_hex(ret);
                crate::console::uart_putc_raw(b'\n');
                if next <= rbp || next >= stack_top {
                    break;
                }
                rbp = next;
            }
            // Forensic ring tail: the last 12 completed syscalls before
            // the crash (pid/nr/a0/ret), oldest first. Identifies the
            // in-flight syscall for interrupt-time crashes. The ring is
            // recorded only under rux:sysring (retired debug=sysring) (perf lane 2026-11) — say
            // so instead of replaying a stale/zero ring.
            if crate::trace::EVENT_SYSRING.armed() {
                use crate::syscall::dispatch::{SYSCALL_CURSOR, SYSCALL_RING};
                let cur = SYSCALL_CURSOR.load(core::sync::atomic::Ordering::Relaxed);
                let len = SYSCALL_RING.len();
                for k in (0..12usize).rev() {
                    let idx = (cur + len - 1 - k) % len;
                    let e = &SYSCALL_RING[idx];
                    for &b in b"kp-sys " {
                        crate::console::uart_putc_raw(b);
                    }
                    kp_put_hex(e.pid.load(core::sync::atomic::Ordering::Relaxed) as u64);
                    crate::console::uart_putc_raw(b' ');
                    kp_put_hex(e.nr.load(core::sync::atomic::Ordering::Relaxed) as u64);
                    crate::console::uart_putc_raw(b' ');
                    kp_put_hex(e.a0.load(core::sync::atomic::Ordering::Relaxed));
                    crate::console::uart_putc_raw(b' ');
                    kp_put_hex(e.ret.load(core::sync::atomic::Ordering::Relaxed));
                    crate::console::uart_putc_raw(b'\n');
                }
            } else {
                for &b in b"kp-sys ring off (boot with rux:sysring (retired debug=sysring))\n" {
                    crate::console::uart_putc_raw(b);
                }
            }
            // KP-RING (the sys_wait4+892 NULL children-walk hunt, 2026-11-21):
            // the faulting task's thread-group ring + each member's
            // children list head/first-node, one line per member, all
            // through the volatile-read primitives (no deref of a poisoned
            // page can fault — the reads ride the same contract as the
            // staleness screens). The one-shot latch is the KP_DUMP_ENTERED
            // latch above; this only ever runs on a dying kernel, so the
            // print volume argument does not apply. Names the free-while-
            // ring-linked producer offline: a member whose children.next
            // is NULL/0 is a page that was zeroed after init — the face.
            {
                use crate::process::task::task_offsets;
                let cur_task: *mut u8 = match crate::sched::current() {
                    Some(c) => (c as *mut crate::process::task::Task) as *mut u8,
                    None => core::ptr::null_mut(),
                };
                fn kp_read64(p: *const u8) -> u64 {
                    if p.is_null() { return 0; }
                    // SAFETY: kernel linear-map diagnostic read, same
                    // contract as the kp-stk scan above (bounded by the
                    // KP_DUMP_ENTERED latch; the page is mapped or the
                    // latch stops the recursion).
                    unsafe { core::ptr::read_volatile(p as *const u64) }
                }
                fn kp_read32(p: *const u8) -> u32 {
                    if p.is_null() { return 0; }
                    unsafe { core::ptr::read_volatile(p as *const u32) }
                }
                let ring_base: usize = cur_task as usize;
                if ring_base == 0 {
                    for &b in b"kp-ring: no current task\n" { crate::console::uart_putc_raw(b); }
                }
                if ring_base != 0 {
                    // cur = the faulting task: walk the ring from its
                    // group leader. All offsets compile-time (offset_of
                    // would be nicer but this module is pre-Task-import;
                    // the exported constants are the maintained truth).
                                        // Thread-group fields: group_leader/next_thread — the
                    // walk heals NULL, so read RAW via offset_of here.
                    const OFF_LEADER: usize = task_offsets::TASK_GROUP_LEADER;
                    const OFF_NEXT: usize = task_offsets::TASK_NEXT_THREAD;
                    const OFF_NR: usize = task_offsets::TASK_NR_THREADS;
                    let leader_p = kp_read64((ring_base + OFF_LEADER) as *const u8) as usize;
                    let leader = if leader_p == 0 || leader_p == ring_base { ring_base } else { leader_p };
                    for &b in b"kp-ring: task=0x" { crate::console::uart_putc_raw(b); }
                    kp_put_hex(ring_base as u64);
                    for &b in b" nr=" { crate::console::uart_putc_raw(b); }
                    kp_put_hex(kp_read32((ring_base + OFF_NR) as *const u8) as u64);
                    crate::console::uart_putc_raw(b'\n');
                    let mut m = leader;
                    let mut guard = 0u32;
                    while m != 0 && guard < 24 {
                        let pid = kp_read32(unsafe { (m as *const u8).add(task_offsets::TASK_PID) } ) as u64;
                        let st = kp_read32(unsafe { (m as *const u8).add(task_offsets::TASK_STATE) } ) as u64;
                        let child_next = kp_read64(unsafe { (m as *const u8).add(task_offsets::TASK_CHILDREN) } );
                        let child_prev = kp_read64(unsafe { (m as *const u8).add(task_offsets::TASK_CHILDREN + 8) } );
                        let sib_next = kp_read64(unsafe { (m as *const u8).add(task_offsets::TASK_SIBLING) } );
                        for &b in b"kp-ring m=" { crate::console::uart_putc_raw(b); }
                        kp_put_hex(m as u64);
                        for &b in b" pid=" { crate::console::uart_putc_raw(b); }
                        kp_put_hex(pid);
                        for &b in b" st=" { crate::console::uart_putc_raw(b); }
                        kp_put_hex(st);
                        for &b in b" ch.nx=" { crate::console::uart_putc_raw(b); }
                        kp_put_hex(child_next);
                        for &b in b" ch.pv=" { crate::console::uart_putc_raw(b); }
                        kp_put_hex(child_prev);
                        for &b in b" sb.nx=" { crate::console::uart_putc_raw(b); }
                        kp_put_hex(sib_next);
                        crate::console::uart_putc_raw(b'\n');
                        let nx = kp_read64(unsafe { (m as *const u8).add(OFF_NEXT) }) as usize;
                        if nx == 0 || nx == m || nx == leader { break; }
                        m = nx;
                        guard += 1;
                    }
                }
            }
            // The old terminal was an unconditional hlt loop — every
            // kernel page fault with no fixup entry died with the whole
            // machine, captured into nothing. The Oops policy (Tranche
            // 5, the riscv64 twin's) decides instead: Linux-shaped
            // report, pstore capture, then either a real panic
            // (interrupt context / oops=panic / idle) or the faulting
            // task dies with SIGSEGV and the machine keeps running.
            crate::debug::oops::oops(
                crate::debug::oops::Kind::PageFault { addr: fault_addr },
                &|w: &mut crate::debug::backtrace::ConsoleWriter| {
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
fn oops_show_regs(w: &mut crate::debug::backtrace::ConsoleWriter, regs: &PtRegs) {
    use core::fmt::Write;
    let _ = write!(
        w,
        "RIP: {:04x}:{}\n",
        regs.cs & 0xffff,
        crate::debug::ksyms::format_symbol(regs.rip)
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
        crate::debug::oops::oops(
            crate::debug::oops::Kind::UnknownException { cause: vector },
            &|w: &mut crate::debug::backtrace::ConsoleWriter| {
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
    use crate::debug::taskdump::{taskdump_dec, taskdump_raw_line};
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

pub const STIME_SLOTS: usize = 24;

pub struct StimeSample {
    pub comm: [core::sync::atomic::AtomicU8; 16],
    pub page: core::sync::atomic::AtomicU64,
    pub count: core::sync::atomic::AtomicU64,
}

pub static STIME_HIST: [StimeSample; STIME_SLOTS] = {
    const EMPTY: StimeSample = StimeSample {
        comm: [const { core::sync::atomic::AtomicU8::new(0) }; 16],
        page: core::sync::atomic::AtomicU64::new(0),
        count: core::sync::atomic::AtomicU64::new(0),
    };
    [EMPTY; STIME_SLOTS]
};

/// Focus table (sc106): launcher-comm samples only — see stime_sample_rip.
pub static STIME_FOCUS: [StimeSample; STIME_SLOTS] = {
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
// from the 1 Hz drift block in the tick path, debug-gated).

pub const UTIME_SLOTS: usize = 32;

pub struct UtimeSample {
    pub page: core::sync::atomic::AtomicU64,
    pub count: core::sync::atomic::AtomicU64,
}

pub static UTIME_HIST: [UtimeSample; UTIME_SLOTS] = {
    const EMPTY: UtimeSample = UtimeSample {
        page: core::sync::atomic::AtomicU64::new(0),
        count: core::sync::atomic::AtomicU64::new(0),
    };
    [EMPTY; UTIME_SLOTS]
};

/// launcher-comm user ticks (mode split numerator).
pub static UTIME_LAUNCHER_TICKS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
/// launcher-comm kernel ticks.
pub static STIME_LAUNCHER_TICKS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

fn comm_has_launcher(comm: &[u8; 16]) -> bool {
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
fn utime_sample_rip(rip: u64, comm: &[u8; 16]) {
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
/// Periodic report (rux:ripsample (retired debug=ripsample)): the launcher's user-rip histogram
/// + utime/stime split + the kernel-mode focus table. Self-throttled
/// to one print every 60 s.
///
/// (arch-cold lane) the report body lives in the arch service
/// crate now; this historical-name wrapper dispatches the cold
/// ArchDiagHooks row.
pub fn utime_ripsample_report() {
    (crate::arch::arch_diag_hooks::arch_diag().utime_ripsample_report)()
}

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
///
/// (arch-cold lane) the formatter body lives in the arch service
/// crate now; this historical-name wrapper dispatches the cold
/// ArchDiagHooks row (the sampler statics and their tick-path writers
/// stay here, published for the crate's readers).
pub fn stime_profile_top(focus: bool) -> alloc::vec::Vec<([u8; 16], u64, u64)> {
    (crate::arch::arch_diag_hooks::arch_diag().stime_profile_top)(focus)
}
