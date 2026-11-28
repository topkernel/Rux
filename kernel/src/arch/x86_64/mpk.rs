//! MPK (Intel pkey / AMD MPK) evaluation probe — Layer 3 of the safety
//! plan (unsafe-5000-plan.md). GATED: runs only with `mpkprobe=1` on
//! the cmdline; default off (zero cost otherwise — one cmdline match
//! at boot).
//!
//! Deliverables (the assignment):
//! 1. Capability check — CPUID.7.0:ECX[3] (OSPKE) / [4] (PKE), plus
//!    the CR4.PKE arm and the PKRU reset-value read.
//! 2. WRPKRU round-trip cost — the ERIM-class number (target: the
//!    plan's <1% overhead budget; measured in TSC cycles per
//!    round-trip on THIS host under KVM).
//! 3. Fault-injection prototype — a kernel BSS page re-keyed to
//!    pkey 1; the probe writes through a raw pointer inside an
//!    exception-table window: with the key ENABLED the write lands
//!    (value verified); with the key DISABLED the write MUST #PF and
//!    the ex_table fixup converts the fault to a return code — the
//!    hardware backstop proven, not argued.
//!
//! The probe page: a dedicated page-aligned static, its PTE in the
//! static kernel root OR'd with pkey bits 62:59 = 1 (map_page_noflush
//! passes raw flag bits straight into the leaf). PKRU's reset value
//! (0x5555_5554) already denies keys 1-15, so between the two probe
//! phases the page is hardware-inaccessible to every context that
//! does not explicitly WRPKRU the key on — exactly the protection
//! shape the plan sketches for the primitives' metadata pages.


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use core::arch::global_asm;

/// The probe's sacrificial page (page-aligned BSS static; its PTE gets
/// the pkey bits).
#[repr(align(4096))]
pub(crate) struct ProbePage(pub(crate) [u8; 4096]);
pub(crate) static mut PROBE_PAGE: ProbePage = ProbePage([0u8; 4096]);

// The fault-injection write: ONE instruction, bracketed by an
// exception-table entry so the protected-region #PF returns 1 instead
// of taking the machine down (the same discipline as the uaccess
// copies). rdi = target; rax = 0 on success, 1 on fault.


extern "C" {
    fn mpk_probe_write(target: *mut u64) -> u32;
}
/// RDPKRU — the protection-key register (EAX = value; ECX/EDX must be
///
/// Collected into `arch::arch_floor` (INV-ARCH-MPK-7): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn rdpkru() -> u32 {
    crate::arch::arch_floor::x86_64::mpk::rdpkru()
}
/// WRPKRU — write PKRU (EAX = value; ECX/EDX must be zero).
///
/// Collected into `arch::arch_floor` (INV-ARCH-MPK-6): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn wrpkru(v: u32) {
    crate::arch::arch_floor::x86_64::mpk::wrpkru(v)
}
/// CR4.PKE (bit 22): arm protection keys. Idempotent.
///
/// Collected into `arch::arch_floor` (INV-ARCH-MPK-5): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) 
fn enable_pke() -> bool {


                        let (_, _, ecx, _) = crate::arch::x86_64::cpu::cpuid(7, 0);
                        if ecx & (1 << 3) == 0 {
                            // OSPKE not exposed (KVM did not pass it through even though
                            // the host has it): the honest negative's primary datum.
                            return false;
                        }
                        // SAFETY: CR4 update with the PKE bit only, CPUID-gated and
                        // idempotent (the enable_smep template).

                            let cr4 = crate::arch::x86_64::cpu::read_cr4();
                            crate::arch::x86_64::cpu::write_cr4(cr4 | (1 << 22));

                        true

}

/// Key 1's bits in PKRU (bit 2 = AD, bit 3 = WD for key 1).
pub(crate) const KEY1_AD: u32 = 1 << 2;
pub(crate) const KEY1_WD: u32 = 1 << 3;
/// The probe entry (called from the x86_64 boot feature block when
///
/// Collected into `arch::arch_floor` (INV-ARCH-MPK-4): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn probe() {
    crate::arch::arch_floor::x86_64::mpk::probe()
}

// ===========================================================================
// MPK adoption phase 1 — the standing guard (mpkguard=1, default off)
// ===========================================================================
//
// Protected target (picked by measured window frequency, the assignment's
// rule): the VRING's AVAIL SECTION — the driver-written pages whose
// avail.idx a stray kernel write corrupts into a permanent I/O wedge
// (queue.rs's own comment names the class). The section is page-isolated
// by construction ("each v1.0 section starts on its own page boundary"),
// so tagging its leaf PTEs pkey=1 touches NOTHING else. Window count =
// one open/close per avail-ring access (per IO submit) ≈ the IO rate
// (~2.4k/s under the pbench fs rows) — versus the GRQ's per-schedule
// window rate (~160k/s under p2_ctxsw: 100k switches in 0.6 s, plus
// enqueues): the vring wins by ~70x, and the plan's other named target
// (GRQ bookkeeping) is recorded as the phase-2 candidate.
//
// Shape (the evaluation's four findings applied):
// - Keys 1-15 armed AD|WD at boot (QEMU's PKRU reset is 0x0 here —
//   finding #3: the reset value cannot be trusted).
// - The avail pages are tagged in the ACTIVE root — the kernel-half
//   PML4 entries are shared LINKS (create_user_address_space), so one
//   leaf tag is instantly visible in every address space.
// - The window: zone_open()/zone_close(prev) with PKRU save/restore
//   (IRQ-nesting-safe: an interrupted open window's close restores the
//   interruptee's value; the scheduler re-arms the closed value at
//   context switch as the preempted-mid-window backstop).
// - The negative probe: probe_escape() writes into a tagged page with
//   the zone CLOSED through an ex_table window — rc=1 proves the
//   standing backstop, not an argued one.

/// Guard armed (mpkguard=1 seen at boot and PKU present).
pub(crate) static GUARD: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Windows opened (the frequency measurement's counter).
pub static WINDOWS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// PKRU with keys 1-15 fully denied (AD|WD) and key 0 open: the guard's
/// standing closed value. = 0xFFFF_FFFC (bits 2..31 set, key0 bits clear).
#[inline]
pub fn closed_pkru() -> u32 {
    0xFFFF_FFFC
}
/// Arm the standing guard (mpkguard=1 at boot). Idempotent; false when
///
/// Collected into `arch::arch_floor` (INV-ARCH-MPK-3): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub 
fn guard_arm() -> bool {


                       let (_, _, ecx, _) = crate::arch::x86_64::cpu::cpuid(7, 0);
                       if ecx & (1 << 3) == 0 {
                           crate::println!("mpkguard: PKU absent — guard NOT armed");
                           return false;
                       }
                       // SAFETY: CR4.PKE arm (idempotent, CPUID-gated — the probe template).

                           let cr4 = crate::arch::x86_64::cpu::read_cr4();
                           crate::arch::x86_64::cpu::write_cr4(cr4 | (1 << 22));

                       wrpkru(closed_pkru());
                       GUARD.store(true, core::sync::atomic::Ordering::Release);
                       crate::println!("mpkguard: armed (keys 1-15 AD|WD, key0 open)");
                       true

}

/// Is the standing guard armed?
#[inline]
pub fn guard_on() -> bool {
    GUARD.load(core::sync::atomic::Ordering::Acquire)
}

/// Open the key-1 zone: returns the PKRU to restore. Every access to a
/// tagged page runs between this and [`zone_close`]. IRQ-nesting-safe
/// (save/restore, not open/close counting).
#[inline]
pub fn zone_open() -> u32 {
    let prev = rdpkru();
    // Clear key1's AD|WD (bits 2,3) only.
    wrpkru(prev & !(KEY1_AD | KEY1_WD));
    WINDOWS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    prev
}

/// Close the zone: restore the PKRU [`zone_open`] returned.
#[inline]
pub fn zone_close(prev: u32) {
    wrpkru(prev);
}

/// The context-switch backstop: re-arm the closed value. A task
/// preempted mid-window cannot leak an open PKRU into the next task —
/// the switch stores the closed value unconditionally (the window's
/// own close then restores... nothing: the interrupted task re-arms on
/// resume via its saved frame? No — PKRU is NOT in the trap frame; the
/// preempted task resumes with whatever the switch left. The window
/// discipline therefore relies on windows never containing a schedule
/// point (they wrap volatile accesses only), and this store closes the
/// hole a kernel-preempt switch inside an open window would otherwise
/// leave. The re-opened access after resume re-faults through the
/// ex_table bracket rather than silently succeeding — fail-closed).
pub fn switch_rearm() {
    if GUARD.load(core::sync::atomic::Ordering::Relaxed) {
        wrpkru(closed_pkru());
    }
}

/// Tag `[va, va+len)` pkey=1 in the ACTIVE root's kernel half. The
/// kernel-half tables are shared links, so this is global. Returns the
/// number of leaves tagged.
pub fn tag_kernel_pages(va: usize, len: usize) -> usize {
    let root = (super::cpu::read_cr3()
        & super::mm::pagetable::PageTableEntry::PHYS_MASK_PUBLIC)
        >> 12;
    let mut tagged = 0usize;
    let mut off = 0usize;
    while off < len {
        let page_va = (va + off) as u64;
        if let Some((ppn, flags)) =
            super::mm::mm_ops::PageTableWalker::walk(root, page_va)
        {
            let phys = super::mm::PhysAddr::new(ppn << 12);
            let ok = super::mm::mmu_init::map_page_noflush(
                root,
                super::mm::VirtAddr::new(page_va),
                phys,
                flags | (1u64 << 59),
            );
            if ok {
                tagged += 1;
            }
        }
        off += 4096;
    }
    tagged
}

/// Untag `[va, va+len)` (clear pkey bits) in the ACTIVE root — the
/// teardown companion of [`tag_kernel_pages`]. Returns leaves cleared.
pub fn untag_kernel_pages(va: usize, len: usize) -> usize {
    let root = (super::cpu::read_cr3()
        & super::mm::pagetable::PageTableEntry::PHYS_MASK_PUBLIC)
        >> 12;
    let mut cleared = 0usize;
    let mut off = 0usize;
    while off < len {
        let page_va = (va + off) as u64;
        if let Some((ppn, flags)) =
            super::mm::mm_ops::PageTableWalker::walk(root, page_va)
        {
            let phys = super::mm::PhysAddr::new(ppn << 12);
            let ok = super::mm::mmu_init::map_page_noflush(
                root,
                super::mm::VirtAddr::new(page_va),
                phys,
                flags & !(0xFu64 << 59),
            );
            if ok {
                cleared += 1;
            }
        }
        off += 4096;
    }
    cleared
}

// The negative probe's escape write: same ex_table shape as the
// evaluation's fault-injection (rc 0 = landed, 1 = hardware #PF).


extern "C" {
    fn mpk_guard_escape(target: *mut u64) -> u32;
}
/// The standing-guard negative test: with the zone CLOSED, a raw write
///
/// Collected into `arch::arch_floor` (INV-ARCH-MPK-1): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn probe_guard_escape(target: *mut u64) -> bool {
    crate::arch::arch_floor::x86_64::mpk::probe_guard_escape(target)
}
