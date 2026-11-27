//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Wild-write producer attribution tripwire (wildwrite-hunt line).
//!
//! The residual wild-write engine (b77 hpet::BASE single-byte smash,
//! wt1 BASE+BASE_LOCKED wide smash, b81 PER_CPU slots) survives every
//! convicted-and-fixed engine (R70 DF, low-half clone, R83/R85 trap.S
//! anchors, mremap interior, 32K stacks). Read-side immunity is in
//! main; this module is the ATTRIBUTION side: catch the store in the
//! act with the writer's rip handed to the existing kernel-#PF printer.
//!
//! Mechanism: page-level read-only guard. Quiet-after-init statics are
//! placed in a page-exclusive cluster (see hpet::GUARD); after boot init
//! this module clears the RW bit on the cluster's PTEs in every alias
//! that can reach the frames —
//!   1. the kernel-half image mapping (the statics' link-time VAs),
//!   2. the linear/direct map alias (PAGE_OFFSET + phys),
//!   3. the still-live low identity window (PML4[0], phys < 2GB),
//! splitting a 2MiB leaf into 4KiB entries where needed.
//! Any later store to those frames page-faults with the writer's RIP,
//! and `kernel/src/arch/x86_64/mm/exception.rs` already prints
//! rip + an rbp frame walk before the KERNPANIC dump — attribution
//! without adding any per-hit infrastructure.
//!
//! Arming happens from dfx::init: AFTER hpet::init has published the
//! statics and BEFORE start_secondaries, so the split needs no TLB
//! shootdown IPIs (APs load the shared kernel root afterwards).
//!
//! A canary page rides in front of the live members: DMA-class writers
//! do not go through PTEs at all, so the RO trap cannot see them. The
//! canary is pattern-filled at arm time and verified from the timer
//! softirq cadence (rux:wildwatch (retired dfx=wildwatch) gates everything; zero cost when off).

#![forbid(unsafe_code)]
#[cfg(feature = "x86_64")]
pub use x86::*;
#[cfg(not(feature = "x86_64"))]
pub use stub::*;

#[cfg(not(feature = "x86_64"))]
mod stub {
    pub fn arm() {}
    pub fn verify() {}
    pub fn is_armed(_addr: usize) -> bool {
        false
    }
}

#[cfg(feature = "x86_64")]
mod x86 {
    use core::sync::atomic::{AtomicU64, Ordering};

    use crate::arch::mm::pagetable::PageTableEntry;

    /// Canary fill qword: "WILDWATC" + per-index salt, complement every
    /// other qword — a partial-line smash still leaves a checkable shape.
    const CANARY_MAGIC: u64 = 0x5749_4c44_5741_5453;

    /// Guarded regions (max 4 clusters x 8 pages each is plenty; the hpet
    /// cluster is 2 pages). Recorded for is_armed() and canary verify.
    const MAX_REGIONS: usize = 8;
    static ARMED_START: [AtomicU64; MAX_REGIONS] =
        [const { AtomicU64::new(0) }; MAX_REGIONS];
    static ARMED_END: [AtomicU64; MAX_REGIONS] =
        [const { AtomicU64::new(0) }; MAX_REGIONS];

    /// True when `addr` falls inside a wildwatch-guarded region (any
    /// alias whose VA range was registered — see arm()).
    pub fn is_armed(addr: usize) -> bool {
        let a = addr as u64;
        for i in 0..MAX_REGIONS {
            let s = ARMED_START[i].load(Ordering::Relaxed);
            if s == 0 {
                continue;
            }
            let e = ARMED_END[i].load(Ordering::Relaxed);
            if a >= s && a < e {
                return true;
            }
        }
        false
    }

    /// Arm all guards. Retried from the timer verify cadence until it
    /// succeeds (observed: at dfx::init time the image PD for the guard
    /// chunk still reads as zeros — parts of the kernel image mapping
    /// settle after dfx::init; the engine only fires minutes in, so a
    /// 1 s-late arming loses nothing). No-op unless rux:wildwatch (retired dfx=wildwatch).
    pub fn arm() {
        static ARM_TRIES: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        static ARMED_FLAG: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(false);
        if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::WildWatch) {
            return;
        }
        if ARMED_FLAG.load(Ordering::Acquire) {
            return;
        }
        let tries = ARM_TRIES.fetch_add(1, Ordering::AcqRel);
        // Fill the hpet cluster's canary page BEFORE RO-ing it (the fill
        // doubles as the mapped-ness probe: it cannot fault).
        let (start, len) = crate::drivers::timer::hpet::wild_guard_range();
        fill_canary(start);
        record(start, len);
        let mut ok_pages = 0usize;
        let mut fail_pages = 0usize;
        let mut guarded_aliases = 0usize;
        // INV-FOR-8 guard edges: the only linking store per split is
        // one aligned 8-byte store; concurrent readers stay consistent
        // under either mapping.
        {
            // Alias 1: link-time kernel-half VAs (the statics themselves).
            for va in (start..start + len).step_by(4096) {
                if crate::dfx::forensics_edges::protect_page_raw(va) {
                    ok_pages += 1;
                } else {
                    fail_pages += 1;
                }
            }
            // Alias 2+3: every other VA the frames are reachable at.
            // Resolve the physical frames through the kernel-half walk,
            // then guard the linear-map alias and (if under 2GB) the low
            // identity alias.
            for va in (start..start + len).step_by(4096) {
                let Some(phys) = crate::dfx::forensics_edges::resolve_phys_raw(va) else {
                    continue;
                };
                let lin = phys
                    .wrapping_add(crate::arch::mm::memory_layout::PAGE_OFFSET as u64);
                if lin != va as u64 && crate::dfx::forensics_edges::protect_page_raw(lin as usize) {
                    guarded_aliases += 1;
                }
                if phys < 0x8000_0000 {
                    if phys as usize != va && crate::dfx::forensics_edges::protect_page_raw(phys as usize) {
                        guarded_aliases += 1;
                    }
                }
            }
        }
        if ok_pages > 0 {
            // CR0.WP: this kernel has never set it (verified live:
            // cr0=0x80000013, WP clear) — with WP=0 a ring-0 store
            // IGNORES the RW bit, so a read-only guard page is writable
            // by the kernel and the trap can never fire. Linux sets WP
            // early for exactly this discipline. Arm it on every CPU
            // that runs this code (BSP here; the verify cadence below
            // re-applies it on the others).
            enable_write_protect();
            ARMED_FLAG.store(true, Ordering::Release);
            crate::pr_info!(
                "dfx: wildwatch armed (try {}): {} guard pages RO ({} failed), {} alias PTEs guarded",
                tries,
                ok_pages,
                fail_pages,
                guarded_aliases
            );
            // Self-test variant: smash immediately in THIS context (the
            // arming CPU, right after its invlpg) — isolates table
            // correctness from cross-CPU TLB questions.
            if crate::cmdline::get_param("wildwatch-selftest").is_some() {
                let victim = (start + 4096) as *mut u64; // GUARD.base_locked
                crate::pr_err!(
                    "WILDWATCH-SELFTEST(arm) firing: store to {:#x} on cpu {}",
                    start + 4096,
                    crate::arch::cpu_id()
                );
                // INV-FOR-5 deliberate-smash edge onto the RO guard
                // page — this instruction MUST page-fault.
                crate::dfx::forensics_edges::diag_write_qword(victim, 0xdead_beef_dead_beef);
                crate::pr_err!("WILDWATCH-SELFTEST(arm) FAULT NOT TAKEN");
            }
        } else if tries < 30 {
            // Retry from the next verify cadence (see verify()).
        } else {
            crate::pr_err!(
                "dfx: wildwatch FAILED to arm after {} tries — guard pages NOT protected",
                tries
            );
            ARMED_FLAG.store(true, Ordering::Release);
        }
    }

    /// Set CR0.WP on the CURRENT cpu (idempotent). With WP clear, ring-0
    /// stores ignore page RW bits — the guard pages would be writable.
    fn enable_write_protect() {
        // INV-FOR-8 CR0 edge: one CR0 read-modify-write; WP is a
        // boot-time-static bit.
        crate::dfx::forensics_edges::cr0_set_write_protect();
    }

    fn fill_canary(start: usize) {
        // Only the FIRST page of the cluster is canary (hpet::GUARD
        // layout: canary page, then the live members).
        for i in 0..512usize {
            let salt = i as u64;
            let v = if i & 1 == 0 {
                CANARY_MAGIC ^ (salt << 32)
            } else {
                !(CANARY_MAGIC ^ (salt << 32))
            };
            // INV-FOR-2 volatile-write edge: kernel .bss page, written
            // once before RO arming.
            crate::dfx::forensics_edges::diag_write_qword(
                (start + i * 8) as *mut u64,
                v,
            );
        }
    }

    /// Verify canary pages (DMA-class writers bypass PTEs; the RO trap
    /// cannot see them). Called from the timer softirq cadence.
    pub fn verify() {
        // Per-CPU idempotent WP arm (APs boot with WP clear; see arm()).
        if ARMED_START[0].load(Ordering::Relaxed) != 0 {
            enable_write_protect();
        }
        // End-to-end self test (cmdline wildwatch-selftest): one cadence
        // after arming, deliberately smash a guarded member. The expected
        // outcome is the full attribution chain: #PF on the RO page ->
        // WILDWRITE-CAUGHT banner -> kernel-fault rip print + rbp walk ->
        // KERNPANIC dump; symbolize the dumped rip with addr2line and it
        // must name THIS function. Boot dies here — that is the design.
        if crate::cmdline::get_param("wildwatch-selftest").is_some() {
            static SELFTEST_FIRED: core::sync::atomic::AtomicBool =
                core::sync::atomic::AtomicBool::new(false);
            let armed = ARMED_START[0].load(Ordering::Relaxed) != 0;
            if armed && !SELFTEST_FIRED.swap(true, Ordering::AcqRel) {
                let (start, _len) = crate::drivers::timer::hpet::wild_guard_range();
                let victim = (start + 4096) as *mut u64; // GUARD.base_locked
                crate::pr_err!("WILDWATCH-SELFTEST firing: deliberate store to {:#x}", start + 4096);
                // INV-FOR-5 deliberate-smash edge onto the RO guard
                // page — this instruction MUST page-fault.
                crate::dfx::forensics_edges::diag_write_qword(victim, 0xdead_beef_dead_beef);
                // If control returns here the trap did NOT fire — the
                // tripwire is broken; scream.
                crate::pr_err!("WILDWATCH-SELFTEST FAULT NOT TAKEN — guard page is NOT read-only!");
            }
        }
        // Retry arming from this cadence until it succeeds (see arm()).
        arm();
        static CANARY_REPORTED: [core::sync::atomic::AtomicBool; MAX_REGIONS] =
            [const { core::sync::atomic::AtomicBool::new(false) }; MAX_REGIONS];
        for r in 0..MAX_REGIONS {
            let s = ARMED_START[r].load(Ordering::Relaxed);
            if s == 0 {
                continue;
            }
            let e = ARMED_END[r].load(Ordering::Relaxed);
            let su = s as usize;
            let canary_end = (su + 4096).min(e as usize);
            for i in 0..((canary_end - su) / 8) {
                let expect = if i & 1 == 0 {
                    CANARY_MAGIC ^ ((i as u64) << 32)
                } else {
                    !(CANARY_MAGIC ^ ((i as u64) << 32))
                };
                // INV-FOR-2 volatile-read edge: registered armed
                // kernel page.
                let v = crate::dfx::forensics_edges::diag_read_qword(su + i * 8);
                if v != expect {
                    if !CANARY_REPORTED[r].swap(true, Ordering::AcqRel) {
                        crate::pr_err!(
                            "WILDWATCH-CANARY addr={:#x} expected={:#x} got={:#x} (PTE-invisible writer: DMA-class?)",
                            su + i * 8,
                            expect,
                            v
                        );
                    }
                    return;
                }
            }
        }
    }

    fn record(start: usize, len: usize) {
        for i in 0..MAX_REGIONS {
            let s = ARMED_START[i].load(Ordering::Relaxed);
            if s == start as u64 {
                return; // already recorded (retry)
            }
            if s == 0 {
                if ARMED_START[i]
                    .compare_exchange(0, start as u64, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    ARMED_END[i].store((start + len) as u64, Ordering::Release);
                }
                return;
            }
        }
    }


    /// Translate a page-table phys to a VA this walk can dereference.
    ///
    /// The arch helper's translation is stage-dependent; at dfx::init a
    /// linear-map read of an image-phys table returned 0 for an entry the
    /// MMU translates fine (PD[3] read 0x0 under a live 2MiB leaf). The
    /// boot-time tables come from the early pool INSIDE the kernel image,
    /// whose kernel-half VAs are valid by the constant VMA-LMA offset —
    /// use that translation for image-phys, arch helper otherwise.
    pub(crate) fn table_virt(phys: u64) -> *mut crate::arch::mm::pagetable::PageTable {
        use crate::arch::mm::memory_layout::{KERNEL_ENTRY, KERNEL_MAP};
        let end = crate::arch::mm::mmu_init::kernel_image_phys_end();
        // INV-FOR-8 edges: the boot-constant VMA-LMA offset (written
        // once during mmu init) and the arch frame translation.
        let offset = crate::dfx::forensics_edges::va_kernel_pa_offset_raw();
        if phys >= KERNEL_ENTRY && phys < end {
            let va = phys.wrapping_add(offset);
            return va as *mut _;
        }
        crate::dfx::forensics_edges::get_page_table_virt_raw(phys)
    }

    /// Resolve the physical address a kernel-half VA maps to (leaf walk).
    /// Full 4-level walk: PML4 -> PDPT -> PD -> PTE.
    fn resolve_phys(va: usize) -> Option<u64> {
        // INV-FOR-8 walk edge: read-only walk of the shared kernel root.
        crate::dfx::forensics_edges::resolve_phys_raw(va)
    }

    /// Clear RW on the 4KiB page covering `va`, splitting a 2MiB leaf
    /// if needed. Returns true when the leaf reads back read-only. The
    /// raw walk/link lives in the audited unsafe file (INV-FOR-8,
    /// forensics_edges::protect_page_raw).
    fn protect_page(va: usize) -> bool {
        crate::dfx::forensics_edges::protect_page_raw(va)
    }
}
