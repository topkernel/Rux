//! The riscv64 trap-path forensic half (arch-cold lane): moved from
//! kernel/core/src/arch/riscv64/trap.rs — the fatal-SIGILL mapping dump
//! (the dying task's full VMA table with perms/type/file attribution
//! for the wild-jump and exec-at-non-exec hunt families). Pure typed
//! walks (vma_read + get_vma_file); no raw reads, no edges needed.
//! The trap handler bodies and the panic printers (putb/phex/pdec,
//! oops_show_regs) stayed framework-side.


/// SIGILL-FORENSIC (gnome-sigill): console dump of the dying task's VMA
/// table + the faulting page's PTE/content. One-shot per death; only runs
/// on the (rare) user-mode illegal-instruction path, so the serial cost is
/// acceptable even for large processes.
pub fn sigill_mapping_forensics(pid: usize, epc: usize) {
    use crate::mm::types::VmaType;

    // One volatile-u64 diagnostic read through the framework's audited
    // riscv64 diag edge (kernel-half page-table/code addresses only).
    fn read_pte(addr: u64) -> u64 {
        crate::arch::riscv64::r#unsafe::diag_volatile_read_u64(addr as usize)
    }

    let task = match crate::sched::hotpath::current() {
        Some(t) => t,
        None => return,
    };
    // Current task's typed handle (trap context).
    let mm = task.address_space_arc();
    let mm = match mm {
        Some(m) => m,
        None => {
            // HOTPATH-PRINT: fatal-path forensics — once per SIGILL death dump
            crate::pr_info!("SIGILL-FORENSIC pid={} no-mm", pid);
            return;
        }
    };

    let page = epc & !0xFFFusize;
    // (start, end, "rwx", "anon|file|dev|shm", contains_epc, file-start or 0)
    let mut rows: alloc::vec::Vec<(usize, usize, alloc::string::String, &'static str, bool, usize)> =
        alloc::vec::Vec::new();
    {
        let vma_mgr = mm.vma_read();
        for vma in vma_mgr.iter() {
            let f = vma.flags();
            let perms = [
                if f.is_readable() { "r" } else { "-" },
                if f.is_writable() { "w" } else { "-" },
                if f.is_executable() { "x" } else { "-" },
            ]
            .concat();
            let ty = match vma.vma_type() {
                VmaType::Anonymous => "anon",
                VmaType::FileBacked => "file",
                VmaType::Device => "dev",
                VmaType::SharedMemory => "shm",
            };
            let contains = epc >= vma.start().as_usize() && epc < vma.end().as_usize();
            let file_start = if vma.vma_type() == VmaType::FileBacked {
                vma.start().as_usize()
            } else {
                0
            };
            rows.push((
                vma.start().as_usize(),
                vma.end().as_usize(),
                perms,
                ty,
                contains,
                file_start,
            ));
        }
    }
    // HOTPATH-PRINT: fatal-path forensics — once per SIGILL death dump
    crate::pr_info!(
        "SIGILL-FORENSIC pid={} epc={:#x} page={:#x} vmas={}",
        pid, epc, page, rows.len()
    );
    for (s, e, perms, ty, contains, file_start) in &rows {
        let path = if *file_start != 0 {
            mm.get_vma_file(*file_start)
                .map(|fl| fl.path())
                .unwrap_or_default()
        } else {
            alloc::string::String::new()
        };
        // HOTPATH-PRINT: fatal-path forensics — once per SIGILL death dump
        crate::pr_info!(
            "  VMA {:#x}-{:#x} {} {} {}{}",
            s, e, perms, ty,
            if *contains { "=> EPC " } else { "" },
            path
        );
    }
    if !rows.iter().any(|r| r.4) {
        // HOTPATH-PRINT: fatal-path forensics — once per SIGILL death dump
        crate::pr_info!("SIGILL-FORENSIC: epc NOT COVERED by any VMA");
    }

    // PTE + page content via a direct page-table walk (independent of the
    // fault path; same technique as coredump's dump_read).
    use crate::arch::mm::{phys_to_virt, PhysAddr};
    let root = mm.root_ppn();
    let page = page as u64;
    let mut pte_addr = (root << 12) + ((page >> 30) & 0x1FF) * 8;
    // Walk reads valid page-table pages of the pinned mm (through the
    // audited diag edge).
    let mut pte = read_pte(pte_addr);
    let mut level = 2u32;
    loop {
        let leaf = pte & 0xE != 0;
        if leaf || level == 0 {
            break;
        }
        let next = ((pte >> 10) & 0xFFF_FFFF_FFFF) << 12;
        let shift = 12 + 9 * (level - 1);
        pte_addr = next + ((page >> shift) & 0x1FF) * 8;
        // Same as above.
        pte = read_pte(pte_addr);
        if pte & 1 == 0 {
            break;
        }
        level -= 1;
    }
    // HOTPATH-PRINT: fatal-path forensics — once per SIGILL death dump
    crate::pr_info!("SIGILL-FORENSIC epc-page PTE @{:#x} = {:#x}", pte_addr, pte);
    // SIGILL-stamp report: is the epc frame one the file-fill path stamped
    // (a genuine text page), and for whom?
    if pte & 1 != 0 {
        let pfn = ((pte >> 10) & 0xFFF_FFFF_FFFF) as usize;
        match crate::mm::hotpath::text_stamp_get(pfn) {
            Some((owner_pid, owner_va)) => {
                // HOTPATH-PRINT: fatal-path forensics — once per SIGILL death dump
                crate::pr_info!(
                    "SIGILL-FORENSIC epc-frame STAMPED pfn={:#x} owner_pid={} owner_va={:#x}",
                    pfn, owner_pid, owner_va
                );
            }
            None => {
                // HOTPATH-PRINT: fatal-path forensics — once per SIGILL death dump
                crate::pr_info!(
                    "SIGILL-FORENSIC epc-frame NOT stamped pfn={:#x} (never a file-fill page, or stamp lost)",
                    pfn
                );
            }
        }
    }
    if pte & 1 == 0 {
        return;
    }
    // Leaf PTE at `level` (0=4K, 1=2M, 2=1G): physical base of the mapping
    // with the low PPN bits (large-page alignment) masked off.
    let leaf_shift = 12 + 9 * level; // bytes covered by this leaf
    let leaf_mask = (1u64 << leaf_shift) - 1;
    let phys_base = ((pte >> 10) << 12) & !leaf_mask;
    let kv_page =
        phys_to_virt(PhysAddr::new(phys_base + (page & leaf_mask))).0 as usize;
    // Page is resident (PTE valid); dump raw words (through the
    // audited diag edge).
    {
        let p64 = kv_page;
        let mut words = [0u64; 8];
        for (i, w) in words.iter_mut().enumerate() {
            *w = read_pte((p64 + i * 8) as u64);
        }
        // HOTPATH-PRINT: fatal-path forensics — once per SIGILL death dump
        crate::pr_info!(
            "SIGILL-FORENSIC page[0..8]: {:016x} {:016x} {:016x} {:016x} {:016x} {:016x} {:016x} {:016x}",
            words[0], words[1], words[2], words[3], words[4], words[5], words[6], words[7]
        );
        let off_words = (((epc as u64) & 0xFFF) as usize / 8).saturating_sub(2);
        let mut w2 = [0u64; 5];
        for (i, w) in w2.iter_mut().enumerate() {
            *w = read_pte((p64 + (off_words + i) * 8) as u64);
        }
        // HOTPATH-PRINT: fatal-path forensics — once per SIGILL death dump
        crate::pr_info!(
            "SIGILL-FORENSIC epc-16..+24: {:016x} {:016x} {:016x} {:016x} {:016x}",
            w2[0], w2[1], w2[2], w2[3], w2[4]
        );
        // SIGILL-FORENSIC full page dump (r9 corruption hunt): 64 rows of
        // 8 u64s — the whole 4 KiB faulting page, so the exact corruption
        // shape (splice points, shifted runs) is recoverable host-side.
        for row in 0..64usize {
            let mut w8 = [0u64; 8];
            for (i, w) in w8.iter_mut().enumerate() {
                *w = read_pte((p64 + (row * 8 + i) * 8) as u64);
            }
            // HOTPATH-PRINT: fatal-path forensics — once per SIGILL death dump
            crate::pr_info!(
                "FDP{:03x}: {:016x} {:016x} {:016x} {:016x} {:016x} {:016x} {:016x} {:016x}",
                row, w8[0], w8[1], w8[2], w8[3], w8[4], w8[5], w8[6], w8[7]
            );
        }
    }
}
