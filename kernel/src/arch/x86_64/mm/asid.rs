//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 PCID layer (gap-sched lane, 2026-11-02).
//!
//! PCID is enabled at bring-up when the CPU reports BOTH PCID
//! (CPUID.1:ECX[17]) and INVPCID (CPUID.7.0:EBX[10]) — the rig's
//! `-cpu host` KVM guests have both. With PCID on:
//!
//! - every `mm` owns a 12-bit PCID (kernel root = PCID 0); `build_satp`
//!   tags CR3 with `PCID | NOFLUSH(bit 63)` so a context switch to a
//!   previously-seen mm does NOT drop cached translations — the CR3
//!   reload (543 ns/switch in the cctx probe) was the largest single
//!   context-switch cost after the FS/GS mirror fixed the MSR pair.
//! - TLB invalidation moves from "CR3 reload flushes everything" to
//!   INVPCID: type 3 for the full flush, type 0 for one page under one
//!   PCID. Plain `invlpg` still flushes the CURRENT PCID's entry, which
//!   is what the fault-path callers want (they run in the faulting mm).
//! - `free_asid` SYNCHRONOUSLY flushes all CPUs before the PCID returns
//!   to the pool: a recycled PCID must never meet stale entries of the
//!   dead mm it last tagged (silent cross-mm corruption on first touch).
//!
//! When either CPUID bit is missing the layer degrades to the old
//! PCID-less behavior: asids are always 0, CR3 writes flush, and
//! flush_* reload CR3. `satp` in these names means CR3.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::super::cpu::{invlpg, read_cr3, write_cr3};
use super::memory_layout::PAGE_SIZE;

pub const ASID_BITS: u64 = 12;
pub const MAX_ASID: u16 = (1 << ASID_BITS) - 1; // 4095
pub const ASID_KERNEL: u16 = 0; // the static kernel PML4's tag
pub const ASID_RESERVED: u16 = 0;
pub const ASID_FIRST: u16 = 1;

// ==================== PCID mode ====================

/// Whether CR4.PCIDE is set and tagged CR3 writes are active.
static PCID_ON: AtomicBool = AtomicBool::new(false);

/// True once `enable_pcid` armed PCID on this boot.
#[inline(always)]
pub fn pcid_enabled() -> bool {
    PCID_ON.load(Ordering::Relaxed)
}

/// CR4.PCIDE bit.
const CR4_PCIDE: u64 = 1 << 17;

/// Arm PCID on the CURRENT cpu (BSP from mmu_init::init tail, APs from
/// their bring-up after switching to the kernel root). Idempotent;
/// silently keeps PCID off (and returns false) when the CPU lacks PCID
/// or INVPCID — every helper here falls back to flush-on-CR3 behavior
/// in that mode, so callers need no branching.
///
/// Architectural precondition: CR0.PG=1 and CR3[11:0]=0 (a #GP would
/// fault otherwise) — both callers load the static kernel root (clean
/// low bits) immediately before.
///
/// PER-CPU DISCIPLINE (same as enable_fsgsbase in cpu.rs): CR4.PCIDE
/// is a per-CPU bit, and every CPU that will EXECUTE a tagged CR3
/// write must arm its own copy. The early return on the global PCID_ON
/// that used to open this function skipped the CR4 arming on every AP
/// (the BSP arms during mmu_init::init, long before start_secondaries)
/// while build_satp kept producing PCID|NOFLUSH-tagged values — on any
/// PCID-exposing vCPU (Intel hosts; this dev rig's AMD host and QEMU's
/// TCG models expose neither CPUID bit, which is why the layer never
/// engaged there) the first user-mm context switch on an AP wrote a
/// bit-63 CR3 with CR4.PCIDE=0, which is architecturally #GP(0).
pub fn enable_pcid() -> bool {
    // CPUID.1:ECX[17] = PCID; CPUID.7.0:EBX[10] = INVPCID. INVPCID is
    // required: without it a full flush cannot reach entries cached
    // under other PCIDs, and the layer must stay off.
    let (_, _, ecx1, _) = crate::arch::x86_64::cpu::cpuid(1, 0);
    let (_, ebx7, _, _) = crate::arch::x86_64::cpu::cpuid(7, 0);
    if ecx1 & (1 << 17) == 0 || ebx7 & (1 << 10) == 0 {
        if !pcid_enabled() {
            crate::println!("mm: PCID off (no CPUID PCID/INVPCID)");
        }
        return false;
    }
    let cr3 = read_cr3();
    if cr3 & 0xfff != 0 {
        // Precondition violated (should never happen from the two call
        // sites): stay PCID-less rather than fault.
        if !pcid_enabled() {
            crate::println!("mm: PCID off (CR3 low bits set at enable)");
        }
        return false;
    }
    let first = !pcid_enabled();
    // SAFETY: single-bit RMW; PG is on and CR3[11:0]=0 checked above.
    unsafe {
        let cr4 = crate::arch::x86_64::cpu::read_cr4();
        crate::arch::x86_64::cpu::write_cr4(cr4 | CR4_PCIDE);
    }
    PCID_ON.store(true, Ordering::Release);
    if first {
        crate::println!("mm: CR4.PCIDE enabled (12-bit PCIDs, INVPCID flushes)");
    }
    true
}

// ==================== PCID allocator ====================
//
// Same lock-free bitmap-CAS shape as the riscv64 twin (512 ASIDs there,
// 4096 PCIDs here). Word 0 starts with PCID 0 reserved for the kernel
// root; ASID_FIRST..=MAX_ASID are the user pool.

/// Words covering MAX_ASID+1 bits (4096 PCIDs -> 64 words).
const ASID_WORDS: usize = (MAX_ASID as usize + 1 + 63) / 64;

static ASID_BITMAP: [AtomicU64; ASID_WORDS] = {
    let mut words = [const { AtomicU64::new(0) }; ASID_WORDS];
    words[0] = AtomicU64::new(0b1); // PCID 0 reserved (kernel root)
    words
};

/// Allocate a PCID.
///
/// Returns the PCID, or None when the pool is exhausted. The caller's
/// fallback (context_switch) loads the root UNTAGGED, which flushes
/// PCID 0 on entry — the legacy behavior, correct but slow, for the
/// >4095-concurrent-mm corner.
pub fn alloc_asid() -> Option<u16> {
    'retry: loop {
        for w in 0..ASID_WORDS {
            let word = ASID_BITMAP[w].load(Ordering::Acquire);
            if word == u64::MAX {
                continue;
            }
            let base = (w * 64) as u16;
            for bit in 0..64u32 {
                let i = base + bit as u16;
                if i < ASID_FIRST || i > MAX_ASID {
                    continue;
                }
                let mask = 1u64 << bit;
                if word & mask == 0 {
                    if ASID_BITMAP[w]
                        .compare_exchange(word, word | mask, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        return Some(i);
                    }
                    continue 'retry; // another CPU claimed it; rescan
                }
            }
        }
        return None;
    }
}

/// Free a PCID, flushing every CPU first.
///
/// SYNCHRONOUS on purpose: entries tagged with this PCID may sit in any
/// CPU's TLB, and the moment the PCID is re-allocated a stale entry
/// would alias the NEW mm's translations onto the DEAD mm's frames. The
/// per-CPU `invpcid all` via smp_call_function waits for each remote
/// flush to execute before the bit is returned to the pool. Cold path
/// (mm teardown), never taken with a PTE lock held (MmStruct::drop
/// frees the asid before walking the tables).
pub fn free_asid(asid: u16) {
    if asid < ASID_FIRST || asid > MAX_ASID {
        return;
    }
    if pcid_enabled() {
        flush_all_cpus_local();
    }
    let w = (asid / 64) as usize;
    let mask = 1u64 << (asid % 64);
    ASID_BITMAP[w].fetch_and(!mask, Ordering::Release);
}

/// The smp_call_function payload for the all-CPU flush above: a plain
/// fn pointer (no closures across FFI), full local invalidation.
fn ipi_flush_all(_arg: *mut core::ffi::c_void) {
    // SAFETY: type 2/3 ignore the descriptor's PCID/address content and
    // are legal regardless of the target's current CR3.
    unsafe { crate::arch::x86_64::cpu::invpcid(crate::arch::x86_64::cpu::INVPCID_ALL_NON_GLOBAL, 0, 0) };
}

/// Full local invalidation plus a SYNCHRONOUS full flush of every other
/// started CPU. Returns when no CPU retains a non-global entry.
fn flush_all_cpus_local() {
    let me = crate::arch::cpu_id() as usize;
    ipi_flush_all(core::ptr::null_mut());
    for cpu in 0..crate::config::MAX_CPUS {
        if cpu != me && crate::arch::smp::cpu_started(cpu) {
            crate::arch::ipi::smp_call_function(cpu, ipi_flush_all, core::ptr::null_mut());
        }
    }
}

/// Live PCIDs (kernel root + allocated user mms).
pub fn asid_usage_count() -> usize {
    let mut used = 0usize;
    for w in 0..ASID_WORDS {
        used += ASID_BITMAP[w].load(Ordering::Acquire).count_ones() as usize;
    }
    used
}

// ==================== TLB flush operations ====================

/// Flush all non-global TLB entries (all PCIDs when PCID is on).
pub fn flush_tlb_all() {
    if pcid_enabled() {
        // SAFETY: type 3 ignores the descriptor and is valid in any CR3.
        unsafe {
            crate::arch::x86_64::cpu::invpcid(crate::arch::x86_64::cpu::INVPCID_ALL_NON_GLOBAL, 0, 0)
        };
    } else {
        // SAFETY: reloading the current CR3 with PCID off flushes
        // non-global translations.
        unsafe { write_cr3(read_cr3()) };
    }
}

/// Flush one PCID's entries everywhere. With PCID off it degrades to
/// the full flush (there is exactly one address space then).
pub fn flush_tlb_asid(asid: u16) {
    if !pcid_enabled() || asid == 0 {
        flush_tlb_all();
        return;
    }
    // Remote CPUs may cache entries under this PCID: synchronous
    // full flush (the cheap single-PCID INVPCID only reaches this CPU).
    flush_all_cpus_local();
}

/// Flush one page of the CURRENT address space (the fault paths run in
/// the faulting mm; cross-PCID flushes go through flush_tlb_all /
/// flush_tlb_asid / the unmap shootdowns).
pub fn flush_tlb_page(addr: u64) {
    // SAFETY: caller passes a canonical address.
    unsafe { invlpg(addr) };
}

/// Flush one page under an explicit PCID (used when the target mm is
/// not the current one). No-op-grade fallback to invlpg with PCID off.
pub fn flush_tlb_page_asid(addr: u64, asid: u16) {
    if pcid_enabled() && asid != 0 {
        // SAFETY: type 0 takes (PCID, linear address) from the descriptor.
        unsafe { crate::arch::x86_64::cpu::invpcid(crate::arch::x86_64::cpu::INVPCID_ADDR, asid as u64, addr) };
    } else {
        flush_tlb_page(addr);
    }
}

/// Flush a range page by page (cold path).
pub fn flush_tlb_range(start: u64, end: u64) {
    let mut a = start & !(PAGE_SIZE - 1);
    while a < end {
        flush_tlb_page(a);
        a += PAGE_SIZE;
    }
}

/// Flush kernel (global) translations — every PCID inclusive.
pub fn flush_tlb_kernel() {
    if pcid_enabled() {
        // SAFETY: type 2 ignores the descriptor content.
        unsafe {
            crate::arch::x86_64::cpu::invpcid(crate::arch::x86_64::cpu::INVPCID_ALL_INCL_GLOBAL, 0, 0)
        };
    } else {
        flush_tlb_all();
    }
}

// ==================== CR3 composition ====================

/// CR3 value for (ppn, asid): the physical root tagged with the PCID
/// and — when PCID is on — bit 63 (NOFLUSH), so the switch keeps both
/// mms' cached translations. bit 63 is only architectural with
/// CR4.PCIDE=1; without it the value is the plain root (and the load
/// flushes, the legacy behavior).
#[inline]
pub fn build_satp(asid: u16, ppn: u64) -> u64 {
    let pcid = if pcid_enabled() { (asid & 0xfff) as u64 } else { 0 };
    let noflush = if pcid_enabled() { 1u64 << 63 } else { 0 };
    (ppn << 12) | pcid | noflush
}

/// PCID field of a live CR3 (0 with PCID off).
pub fn satp_to_asid(cr3: u64) -> u16 {
    if pcid_enabled() {
        (cr3 & 0xfff) as u16
    } else {
        0
    }
}

/// Root PPN of a CR3 (masks the PCID and NOFLUSH bits).
pub fn satp_to_ppn(cr3: u64) -> u64 {
    (cr3 >> 12) & 0x000F_FFFF_FFFF_F
}

/// Activate `ppn` under `asid` on this CPU — the single switch-to-root
/// discipline shared by enable()/disable() and the exec/exit paths:
/// - PCID'd root (asid != 0): tagged NOFLUSH write (keep cached
///   translations);
/// - kernel root (asid 0 under PCID): tagged NOFLUSH write (its entries
///   are only invalidated by the explicit kernel flush sites);
/// - user root WITHOUT a PCID (allocator exhausted, PCID on): plain
///   write — the load flushes PCID 0 so a previous fallback root's
///   entries cannot leak into this one;
/// - PCID off: plain write (the legacy flush-on-load).
///
/// # Safety
/// `ppn` must be a live page-table root PPN.
pub unsafe fn switch_root(ppn: u64, asid: u16) {
    if asid != 0 || !pcid_enabled() {
        // SAFETY: caller guarantees a valid root; tagging per fn doc.
        unsafe { write_cr3(build_satp(asid, ppn)) };
    } else {
        // SAFETY: plain load of a valid root; flushes on entry (only
        // reachable with PCID on for a PCID-less user root).
        unsafe { write_cr3(ppn << 12) };
    }
}

pub fn read_satp() -> u64 {
    read_cr3()
}

/// # Safety
/// `cr3` must be a valid PML4 physical address (tagged form allowed).
pub unsafe fn write_satp(cr3: u64) {
    write_cr3(cr3)
}

/// ASID context (interface parity)
pub struct AsidContext;
impl AsidContext {
    pub const fn new() -> Self {
        AsidContext
    }
}

pub fn print_asid_status() {
    if pcid_enabled() {
        crate::println!("mm: PCID active, {} / 4096 in use", asid_usage_count());
    } else {
        crate::println!("mm: PCID inactive (flush-on-CR3 mode)");
    }
}
