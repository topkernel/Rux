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
    // SAFETY: pure register write; serializing per the ISA.
    unsafe {
        core::arch::asm!(
            "xor ecx, ecx",
            "xor edx, edx",
            "wrpkru",
            in("eax") v,
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
    // Re-key the probe page's PTE to pkey 1.
    // SAFETY: the static's address is stable; the walk/map pair below
    // rewrites only its leaf PTE in the static kernel root.
    let va = unsafe { core::ptr::addr_of_mut!(PROBE_PAGE) } as usize as u64;
    let root = super::mm::mmu_init::root_page_table_ppn();
    let Some((ppn, flags)) = super::mm::mm_ops::PageTableWalker::walk(root, va) else {
        crate::println!("mpk: probe page walk FAILED (va={:#x})", va);
        return;
    };
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
