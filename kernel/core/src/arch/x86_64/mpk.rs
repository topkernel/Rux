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

use core::arch::global_asm;

/// The probe's sacrificial page (page-aligned BSS static; its PTE gets
/// the pkey bits).
#[repr(align(4096))]
struct ProbePage([u8; 4096]);
static mut PROBE_PAGE: ProbePage = ProbePage([0u8; 4096]);

// The fault-injection write: ONE instruction, bracketed by an
// exception-table entry so the protected-region #PF returns 1 instead
// of taking the machine down (the same discipline as the uaccess
// copies). rdi = target; rax = 0 on success, 1 on fault.
global_asm!(
    ".global mpk_probe_write",
    ".hidden mpk_probe_write",
    "mpk_probe_write:",
    "0: movq $0x5A5A5A5A, (%rdi)",
    "   xorl %eax, %eax",
    "   ret",
    "1: movl $1, %eax",
    "   ret",
    ".pushsection __ex_table, \"a\"",
    ".balign 8",
    ".quad 0b",
    ".quad 1b",
    ".popsection",
    options(att_syntax)
);

extern "C" {
    fn mpk_probe_write(target: *mut u64) -> u32;
}

/// RDPKRU — the protection-key register (EAX = value; ECX/EDX must be
/// zero per the ISA).
#[inline]
fn rdpkru() -> u32 {
    let v: u32;
    // SAFETY: pure register read (the F-REG class: no memory effect).
    unsafe {
        core::arch::asm!(
            "xor ecx, ecx",
            "xor edx, edx",
            "rdpkru",
            lateout("eax") v,
            out("ecx") _,
            out("edx") _,
            options(nostack)
        );
    }
    v
}

/// WRPKRU — write PKRU (EAX = value; ECX/EDX must be zero).
#[inline]
fn wrpkru(v: u32) {
    // SAFETY: pure register write; serializing per the ISA. ECX/EDX
    // are DECLARED CLOBBERED (out(_), like rdpkru): the cost-loop bug
    // — the template's "xor ecx,ecx" ran undeclared, so the release
    // compiler legally kept a loop induction variable in ECX across
    // the asm; `dec ecx; jne` then counted 0 -> 0xFFFFFFFF and the
    // probe's warmup loop became ~2^32 iterations (a boot hang with
    // zero output — RIP convicted inside the wrpkru loop via the QEMU
    // monitor). An undeclared clobber is UB in EVERY caller context.
    unsafe {
        core::arch::asm!(
            "xor ecx, ecx",
            "xor edx, edx",
            "wrpkru",
            in("eax") v,
            out("ecx") _,
            out("edx") _,
            options(nostack)
        );
    }
}

/// CR4.PKE (bit 22): arm protection keys. Idempotent.
fn enable_pke() -> bool {
    let (_, _, ecx, _) = super::cpu::cpuid(7, 0);
    if ecx & (1 << 3) == 0 {
        // OSPKE not exposed (KVM did not pass it through even though
        // the host has it): the honest negative's primary datum.
        return false;
    }
    // SAFETY: CR4 update with the PKE bit only, CPUID-gated and
    // idempotent (the enable_smep template).
    unsafe {
        let cr4 = super::cpu::read_cr4();
        super::cpu::write_cr4(cr4 | (1 << 22));
    }
    true
}

/// Key 1's bits in PKRU (bit 2 = AD, bit 3 = WD for key 1).
const KEY1_AD: u32 = 1 << 2;
const KEY1_WD: u32 = 1 << 3;

/// The probe entry (called from the x86_64 boot feature block when
/// cmdline carries mpkprobe=1).
pub fn probe() {
    // CPUID.(7,0):ECX bit 3 = PKU (hardware support), bit 4 = OSPKE
    // (mirrors CR4.PKE — reads 0 until WE set it; the first boot had
    // this backwards and read a working CPU as "not available").
    let (_, _, ecx_pre, _) = super::cpu::cpuid(7, 0);
    let pku = ecx_pre & (1 << 3) != 0;
    crate::println!("mpk: CPUID.7.0:ECX={:#x} PKU(hw)={} OSPKE(pre-arm)={}", ecx_pre, pku as u8, (ecx_pre & (1 << 4) != 0) as u8);
    if !pku {
        crate::println!("mpk: NOT AVAILABLE in this guest — the Layer-3 negative (see unsafe-5000-plan.md)");
        return;
    }
    if !enable_pke() {
        crate::println!("mpk: CR4.PKE arm failed");
        return;
    }
    // OSPKE must now read 1 (the CR4.PKE mirror) — the arm's own
    // verification.
    let (_, _, ecx_post, _) = super::cpu::cpuid(7, 0);
    let ospke = ecx_post & (1 << 4) != 0;
    let reset_pkru = rdpkru();
    crate::println!("mpk: CR4.PKE armed — OSPKE(post)={}", ospke as u8);
    crate::println!("mpk: CR4.PKE armed, PKRU reset={:#x} (key0 full, keys1-15 AD)", reset_pkru);

    // ---- WRPKRU round-trip cost (the ERIM-class number) ----
    // 100k round-trips of write-the-same-value, TSC-timed; report
    // cycles/op (median-of-batches would be overkill for a register
    // write — total/n with a warmup).
    let tsc = |lo: &mut u32, hi: &mut u32| {
        // SAFETY: RDTSC (the pbench probe's own clock class).
        unsafe {
            core::arch::asm!("rdtsc", lateout("eax") *lo, lateout("edx") *hi, options(nostack));
        }
    };
    let mut lo = 0u32;
    let mut hi = 0u32;
    // warmup
    for _ in 0..1000 {
        wrpkru(reset_pkru);
    }
    tsc(&mut lo, &mut hi);
    let t0 = (hi as u64) << 32 | lo as u64;
    const N: u64 = 100_000;
    for _ in 0..N {
        wrpkru(reset_pkru);
    }
    tsc(&mut lo, &mut hi);
    let t1 = (hi as u64) << 32 | lo as u64;
    let cycles_per = (t1 - t0) / N;
    crate::println!(
        "mpk: WRPKRU round-trip = {} cycles/op ({} ops in {} cycles)",
        cycles_per,
        N,
        t1 - t0
    );

    // ---- Fault-injection prototype ----
    // Map a FRESH page at an unused fixmap slot with pkey 1 bits (the
    // BSS-page walk failed because the kernel image lives in 2MB huge
    // leaves — the walker's table descent can't reach a 4K PTE there;
    // a fresh 4K fixmap install avoids the demotion entirely).
    // A fresh USER-range VA (not fixmap): the last fixmap slot's PMD
    // held a huge/foreign leaf — the 4K walk resolved garbage (the
    // pte_pre diag). User range is guaranteed 4K-leaved on this root,
    // and a kernel-mode write to a US page is legal with SMAP unarmed
    // (the boot arms SMEP only) — PKU still gates it via PKRU.
    let va: u64 = 0x6000_0000_0000;
    let frame = crate::mm::hotpath::alloc_pages(crate::mm::types::GfpFlags::GFP_KERNEL, 0);
    if frame == 0 {
        crate::println!("mpk: probe page alloc FAILED");
        return;
    }
    let ppn = (frame >> 12) as u64;
    let flags: u64 = 0x7; // PRESENT | RW | USER
    // The ACTIVE root (CR3), not the static one: the probe runs in a
    // live task context whose CR3 is the init task's cloned root — a
    // static-root-only install left the page absent, phase A's write
    // faulted, and the kernel's kernel-VA fault path re-mapped it on
    // its own NX template (the pte diag: NX|A|D, no pkey bits — a
    // template-override, not a PKU enforcement failure).
    let root = (super::cpu::read_cr3()
        & super::mm::pagetable::PageTableEntry::PHYS_MASK_PUBLIC)
        >> 12;
    // pkey bits 62:59 = 1 (the leaf install ORs raw flag bits).
    let pkey_bits: u64 = 1 << 59;
    let phys = super::mm::PhysAddr::new(ppn << 12);
    let ok = super::mm::mmu_init::map_page_noflush(
        root,
        super::mm::VirtAddr::new(va),
        phys,
        flags | pkey_bits,
    );
    if !ok {
        crate::println!("mpk: probe page re-key FAILED");
        return;
    }
    // invlpg on the probe VA (the map's own flush discipline —
    // map_page_noflush skips it, we do it once here).
    super::cpu::invlpg(va);
    let target = va as *mut u64;

    // Phase A: key ENABLED (clear AD|WD for key 1) — the write must land.
    wrpkru(reset_pkru & !(KEY1_AD | KEY1_WD));
    let rc_a = unsafe { mpk_probe_write(target) };
    let val = unsafe { core::ptr::read_volatile(target) };
    crate::println!(
        "mpk: fault-inject phase A (key1 enabled): rc={} value={:#x} -> {}",
        rc_a,
        val,
        if rc_a == 0 && val == 0x5A5A5A5A { "PASS (write landed)" } else { "FAIL" }
    );

    // Phase B: key DISABLED (AD set) — the SAME write must #PF into the
    // ex_table fixup: the hardware backstop proven.
    wrpkru(reset_pkru | KEY1_AD | KEY1_WD);
    let rc_b = unsafe { mpk_probe_write(target) };
    crate::println!(
        "mpk: fault-inject phase B (key1 disabled): rc={} -> {}",
        rc_b,
        if rc_b == 1 { "PASS (hardware #PF, ex_table fixup)" } else { "FAIL (no fault!)" }
    );

    // Restore: key back to enabled-zero effect and the PTE's pkey
    // bits cleared (leave the machine exactly as the probe found it).
    wrpkru(reset_pkru & !(KEY1_AD | KEY1_WD));
    let _ = super::mm::mmu_init::map_page_noflush(
        root,
        super::mm::VirtAddr::new(va),
        phys,
        flags,
    );
    // Same one-shot flush.
    super::cpu::invlpg(va);
    wrpkru(reset_pkru);
    crate::println!("mpk: probe complete (page un-keyed, PKRU restored)");
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
static GUARD: core::sync::atomic::AtomicBool =
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
/// the hardware bit is absent (the guard silently stays off — the
/// cmdline asked for protection the machine cannot give, say so).
pub fn guard_arm() -> bool {
    let (_, _, ecx, _) = super::cpu::cpuid(7, 0);
    if ecx & (1 << 3) == 0 {
        crate::println!("mpkguard: PKU absent — guard NOT armed");
        return false;
    }
    // SAFETY: CR4.PKE arm (idempotent, CPUID-gated — the probe template).
    unsafe {
        let cr4 = super::cpu::read_cr4();
        super::cpu::write_cr4(cr4 | (1 << 22));
    }
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

/// Per-CPU arm at AP bringup (ap_entry64): CR4.PKE and the PKRU are
/// PER-CPU state — the BSP's [`guard_arm`] arms only the BSP, and any
/// RDPKRU/WRPKRU on an un-armed AP #UDs (kernel SIGILL on the AP's
/// first context switch, `switch_rearm`'s wrpkru: the mab-grd1 boot —
/// CPU 1, PID 0, RAX=closed_pkru, handle_illegal_instruction ->
/// "Attempted to kill the idle task"). The tagged kernel-PT leaves are
/// shared, so only the register arm is per-CPU. Runs before the AP's
/// first STI and schedule; no-op when the guard is off.
pub fn cpu_online_arm() {
    if !guard_on() {
        return;
    }
    let (_, _, ecx, _) = super::cpu::cpuid(7, 0);
    if ecx & (1 << 3) == 0 {
        crate::println!(
            "mpkguard: cpu{} has no PKU — guard NOT armed here",
            crate::arch::cpu_id()
        );
        return;
    }
    // SAFETY: CR4 update with the PKE bit only, CPUID-gated and
    // idempotent (the guard_arm template).
    unsafe {
        let cr4 = super::cpu::read_cr4();
        super::cpu::write_cr4(cr4 | (1 << 22));
    }
    wrpkru(closed_pkru());
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
            // Strip PS before the 4K re-install: the walk (fixed)
            // resolves 2MB/1GB leaves, whose leaf bits carry PS — a
            // PTE with PS set is reserved-bit-invalid at the 4K level
            // (the page would read unmapped after tagging).
            let ok = super::mm::mmu_init::map_page_noflush(
                root,
                super::mm::VirtAddr::new(page_va),
                phys,
                (flags & !super::mm::pagetable::PageTableEntry::PS)
                    | (1u64 << 59),
            );
            if ok {
                // Invalidate the page's translation NOW: the page was
                // very likely touched before tagging (allocator poison
                // paint / vring setup writes), and without the invlpg a
                // stale pkey-0 TLB entry keeps serving accesses — the
                // very first escape probe convicted this (rc=0, write
                // landed). Tagging runs on the BSP before the APs come
                // online, so a local invlpg covers every CPU that can
                // hold a stale entry; an SMP-time re-tag needs the
                // broadcast shootdown instead (phase 2).
                super::cpu::invlpg(page_va);
                tagged += 1;
            }
        }
        off += 4096;
    }
    if tagged == 0 && len > 0 {
        // Tripwire: a zero-tag result means the windowed accessors stay
        // dormant for this block (fail-open to untagged, never
        // fail-closed on live traffic) — name it on every boot rather
        // than letting the guard silently protect nothing.
        crate::println!(
            "mpkguard: tag returned 0 for {:#x}+{:#x}",
            va, len
        );
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
            // PS strip: same as tag_kernel_pages — leaf-resolved flags
            // must not carry PS into the 4K re-install.
            let ok = super::mm::mmu_init::map_page_noflush(
                root,
                super::mm::VirtAddr::new(page_va),
                phys,
                (flags & !super::mm::pagetable::PageTableEntry::PS)
                    & !(0xFu64 << 59),
            );
            if ok {
                // Same invlpg discipline as tag_kernel_pages: the
                // untagged PTE must displace any cached translation.
                super::cpu::invlpg(page_va);
                cleared += 1;
            }
        }
        off += 4096;
    }
    cleared
}

// The negative probe's escape write: same ex_table shape as the
// evaluation's fault-injection (rc 0 = landed, 1 = hardware #PF).
global_asm!(
    ".global mpk_guard_escape",
    ".hidden mpk_guard_escape",
    "mpk_guard_escape:",
    "0: movq $0xDEAD, (%rdi)",
    "   xorl %eax, %eax",
    "   ret",
    "1: movl $1, %eax",
    "   ret",
    ".pushsection __ex_table, \"a\"",
    ".balign 8",
    ".quad 0b",
    ".quad 1b",
    ".popsection",
    options(att_syntax)
);

extern "C" {
    fn mpk_guard_escape(target: *mut u64) -> u32;
}

/// The standing-guard negative test: with the zone CLOSED, a raw write
/// into a tagged page must #PF (rc=1). Returns the pass verdict text.
pub fn probe_guard_escape(target: *mut u64) -> bool {
    // SAFETY: the ex_table bracket converts the expected #PF to rc=1;
    // if it unexpectedly lands, rc=0 = the guard is NOT standing.
    let rc = unsafe { mpk_guard_escape(target) };
    crate::println!(
        "mpkguard: escape probe (zone closed) rc={} -> {}",
        rc,
        if rc == 1 { "PASS (hardware backstop standing)" } else { "FAIL (write landed!)" }
    );
    rc == 1
}
