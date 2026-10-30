//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! E8 regression tests: process-tree reparent safety (GNOME final8
//! kernel panic at guest+3286s — dangling parent pointers walked by
//! reparent_children_to_init) and the megapage-demotion mmap fix
//! (gnome-shell gjs/mozjs heap-cage SIGSEGV at guest+998s).

use crate::println;
use crate::process::Task;
use crate::process::task::SchedPolicy;
use crate::sched::sched::TASK_POISON;
use alloc::boxed::Box;
use super::{test_pass, test_fail, test_skip, test_group_start};

/// Task::is_plausible_task_ptr — the per-hop screen the subreaper walk
/// now applies before every dereference.
pub fn test_plausible_task_ptr() {
    test_group_start("is_plausible_task_ptr");

    // Null and user-range pointers (the final8 panic faulted dereferencing
    // memory reached through a value like 0xb5d66) must be rejected.
    if !Task::is_plausible_task_ptr(core::ptr::null()) {
        test_pass("null pointer rejected");
    } else {
        test_fail("null pointer rejected", "null accepted");
    }
    if !Task::is_plausible_task_ptr(0xb5d66 as *const Task) {
        test_pass("user-range pointer rejected");
    } else {
        test_fail("user-range pointer rejected", "0xb5d66 accepted");
    }
    if !Task::is_plausible_task_ptr(0xFFFF_FFFF_FFFF_FFFF as *const Task) {
        test_pass("u32::MAX sentinel rejected");
    } else {
        test_fail("u32::MAX sentinel rejected", "sentinel accepted");
    }

    // A live, freshly built Task must pass.
    let mut t = Box::new(Task::new(4242, SchedPolicy::Normal));
    t.children.init();
    t.sibling.init();
    if Task::is_plausible_task_ptr(&*t as *const Task) {
        test_pass("live task accepted");
    } else {
        test_fail("live task accepted", "live task rejected");
    }

    // A freed Task (poisoned state/pid at the compile-time offsets, exactly
    // what free_task_slot writes) must be rejected.
    let base = &mut *t as *mut Task as *mut u8;
    unsafe {
        core::ptr::write_volatile(
            base.add(crate::process::task::task_offsets::TASK_STATE) as *mut u32,
            TASK_POISON,
        );
        core::ptr::write_volatile(
            base.add(crate::process::task::task_offsets::TASK_PID) as *mut u32,
            TASK_POISON,
        );
    }
    if !Task::is_plausible_task_ptr(&*t as *const Task) {
        test_pass("poisoned task rejected");
    } else {
        test_fail("poisoned task rejected", "poison accepted");
    }
}

/// Task::move_child — atomic validate + unlink + relink. The race window
/// the old remove_child()/add_child() pair left open (concurrent reaper
/// freeing the child between the two locked sections) must now be a
/// clean "skip" (false), never a write into the child.
pub fn test_move_child() {
    test_group_start("move_child");

    let mut dying_b = Box::new(Task::new(100, SchedPolicy::Normal));
    let mut dest_b = Box::new(Task::new(1, SchedPolicy::Normal));
    let mut child_b = Box::new(Task::new(101, SchedPolicy::Normal));
    dying_b.children.init();
    dying_b.sibling.init();
    dest_b.children.init();
    dest_b.sibling.init();
    child_b.children.init();
    child_b.sibling.init();
    let dying = Box::leak(dying_b) as *mut Task;
    let dest = Box::leak(dest_b) as *mut Task;
    let child = Box::leak(child_b) as *mut Task;

    unsafe {
        // Not linked yet: move must refuse (child is not ours).
        if !(*dying).move_child(child, dest) {
            test_pass("unlinked child refused");
        } else {
            test_fail("unlinked child refused", "moved an unlinked child");
        }

        (*dying).add_child(child);

        // Simulate the concurrent reaper: unlink under the tree lock the
        // way release_task/claim does, then move — must refuse.
        {
            let _lock = crate::process::task::PROCESS_TREE_LOCK.lock();
            if Task::claim_child_locked(child) {
                test_pass("claim of linked child succeeds");
            } else {
                test_fail("claim of linked child succeeds", "claim failed");
            }
        }
        if !(*dying).move_child(child, dest) {
            test_pass("concurrently-unlinked child refused");
        } else {
            test_fail("concurrently-unlinked child refused", "moved a claimed child");
        }

        // Re-link and do the real move: parent, list head and list
        // membership must all transfer in one shot.
        (*dying).add_child(child);
        if (*dying).move_child(child, dest) {
            test_pass("linked child moved");
        } else {
            test_fail("linked child moved", "move refused a linked child");
        }
        if (*child).parent_ptr() == Some(dest as *const Task) {
            test_pass("parent pointer updated");
        } else {
            test_fail("parent pointer updated", "parent not dest");
        }
        let mut on_dest = false;
        let mut dest_count = 0usize;
        (*dest).for_each_child(|c| {
            dest_count += 1;
            if c == child { on_dest = true; }
        });
        if on_dest && dest_count == 1 {
            test_pass("child linked into dest list exactly once");
        } else {
            test_fail("child linked into dest list exactly once", "list state wrong");
        }
        let mut dying_count = 0usize;
        (*dying).for_each_child(|_| dying_count += 1);
        if dying_count == 0 {
            test_pass("dying list empty after move");
        } else {
            test_fail("dying list empty after move", "dying still has children");
        }

        // Cleanup: unlink from dest, then drop the heap Tasks. free via Box.
        let _lock = crate::process::task::PROCESS_TREE_LOCK.lock();
        let claimed = Task::claim_child_locked(child);
        drop(_lock);
        if claimed {
            test_pass("cleanup claim");
        }
    }
    unsafe {
        drop(Box::from_raw(dying));
        drop(Box::from_raw(dest));
        drop(Box::from_raw(child));
    }
}

/// Task::claim_child_locked — exactly one claimer wins (the do_wait
/// double-reap guard: two group members woken by the same SIGCHLD must
/// not both release_task the same zombie).
pub fn test_claim_child() {
    test_group_start("claim_child_locked");

    let mut parent_b = Box::new(Task::new(200, SchedPolicy::Normal));
    let mut z1_b = Box::new(Task::new(201, SchedPolicy::Normal));
    let mut z2_b = Box::new(Task::new(202, SchedPolicy::Normal));
    parent_b.children.init();
    parent_b.sibling.init();
    z1_b.children.init();
    z1_b.sibling.init();
    z2_b.children.init();
    z2_b.sibling.init();
    let parent = Box::leak(parent_b) as *mut Task;
    let z1 = Box::leak(z1_b) as *mut Task;
    let z2 = Box::leak(z2_b) as *mut Task;

    unsafe {
        (*parent).add_child(z1);
        (*parent).add_child(z2);

        let (c1, c2, c3) = {
            let _lock = crate::process::task::PROCESS_TREE_LOCK.lock();
            // First scanner claims z1; a second scanner must fail.
            let c1 = Task::claim_child_locked(z1);
            let c2 = Task::claim_child_locked(z1);
            // The other zombie is untouched.
            let c3 = Task::claim_child_locked(z2);
            (c1, c2, c3)
        };
        if c1 && !c2 && c3 {
            test_pass("exactly one claimer wins per child");
        } else {
            test_fail("exactly one claimer wins per child", "claim protocol broken");
        }
        if (*z1).parent_ptr().is_none() && (*z2).parent_ptr().is_none() {
            test_pass("claims cleared parent pointers");
        } else {
            test_fail("claims cleared parent pointers", "parent left set");
        }
        let mut remaining = 0usize;
        (*parent).for_each_child(|_| remaining += 1);
        if remaining == 0 {
            test_pass("parent list drained by claims");
        } else {
            test_fail("parent list drained by claims", "children left on list");
        }

        drop(Box::from_raw(parent));
        drop(Box::from_raw(z1));
        drop(Box::from_raw(z2));
    }
}

/// kernel_device_window_pte — the VA table clear_pte consults to RESTORE
/// a device translation when a user mapping over a low-half device
/// window (PLIC/ECAM/PCI-MMIO/MMIO pages) is unmapped.
pub fn test_device_window_pte() {
    test_group_start("kernel_device_window_pte");

    // aarch64 stage-2: aarch64's kernel_device_window_pte is a None stub
    // (arch/aarch64/mm/mmu_init.rs — TTBR1 device-window walk pending,
    // mm lane). Skip until that lands; riscv64/x86_64 keep the pins.
    #[cfg(feature = "aarch64")]
    {
        test_skip(
            "kernel_device_window_pte",
            "aarch64: device-window walk pending (mm stage-2)",
        );
        return;
    }

    use crate::arch::mm::mmu_init::kernel_device_window_pte;
    #[cfg(not(feature = "x86_64"))]
    use crate::arch::mm::PageTableEntry;

    // Per-arch probe addresses (each arch's QEMU machine map):
    // - riscv64 virt: PLIC 0x0c00_0000 (priority) / 0x0c20_1000 (S-mode
    //   ctx-1 claim), ECAM 0x3000_0000, PCI MMIO 0x4000_0000.
    // - x86_64 q35: ECAM (MMCONFIG) 0xb000_0000, 32-bit PCI MMIO hole
    //   0xc000_0000..4GB (arch/x86_64/mm/mmu_init.rs WINDOW list). No
    //   PLIC exists there, so the restore-bits pin (which asserts the
    //   riscv64 PLIC PTE shape) stays riscv64-only.
    #[cfg(not(feature = "x86_64"))]
    const IRQCTRL_PAGE: u64 = 0x0c00_0000; // PLIC priority page
    #[cfg(not(feature = "x86_64"))]
    const IRQCTRL_PAGE2: u64 = 0x0c20_1000; // PLIC claim page
    #[cfg(not(feature = "x86_64"))]
    const PCI_MMIO_PAGE: u64 = 0x4000_0000;
    #[cfg(not(feature = "x86_64"))]
    const ECAM_PAGE: u64 = 0x3000_0000;
    #[cfg(not(feature = "x86_64"))]
    const INTERIOR_A: u64 = 0x0c00_1000;
    #[cfg(not(feature = "x86_64"))]
    const INTERIOR_B: u64 = 0x0c20_FFFF;

    #[cfg(feature = "x86_64")]
    const IRQCTRL_PAGE: u64 = 0xb000_0000; // ECAM head doubles as window pin
    #[cfg(feature = "x86_64")]
    const IRQCTRL_PAGE2: u64 = 0xb000_1000; // ECAM interior
    #[cfg(feature = "x86_64")]
    const PCI_MMIO_PAGE: u64 = 0xc000_0000; // 32-bit MMIO hole head
    #[cfg(feature = "x86_64")]
    const ECAM_PAGE: u64 = 0xb000_0000;
    #[cfg(feature = "x86_64")]
    const INTERIOR_A: u64 = 0xc000_1000; // MMIO-hole interior
    #[cfg(feature = "x86_64")]
    const INTERIOR_B: u64 = 0xc000_FFFF;

    // Assertion names (riscv64 strings unchanged byte-for-byte; x86_64
    // names its own windows — no PLIC there).
    #[cfg(not(feature = "x86_64"))]
    const N_HEAD: &str = "PLIC priority page restores device PTE";
    #[cfg(not(feature = "x86_64"))]
    const N_HEAD2: &str = "PLIC claim page covered";
    #[cfg(feature = "x86_64")]
    const N_HEAD: &str = "device window head restores device PTE";
    #[cfg(feature = "x86_64")]
    const N_HEAD2: &str = "device window second page covered";

    // IRQ-controller window head: covered, identity-mapped, kernel
    // flags, no U (riscv64 asserts the full restore-bit shape; x86_64
    // only the coverage — its PTE flag encoding is the x86 lane's pin).
    match kernel_device_window_pte(IRQCTRL_PAGE) {
        Some(bits) => {
            #[cfg(not(feature = "x86_64"))]
            {
                let pte = PageTableEntry::from_bits(bits);
                if pte.is_valid() && pte.is_readable() && pte.is_writable() && !pte.is_user()
                    && pte.ppn() == (IRQCTRL_PAGE >> 12)
                {
                    test_pass(N_HEAD);
                } else {
                    test_fail(N_HEAD, "wrong restore bits");
                }
            }
            #[cfg(feature = "x86_64")]
            test_pass(N_HEAD);
        }
        None => test_fail(N_HEAD, "page not covered"),
    }

    // IRQ-controller window second page: must also restore.
    match kernel_device_window_pte(IRQCTRL_PAGE2) {
        Some(_) => test_pass(N_HEAD2),
        None => test_fail(N_HEAD2, "claim page not covered"),
    }

    // PCI MMIO window start and ECAM window start covered.
    if kernel_device_window_pte(PCI_MMIO_PAGE).is_some() {
        test_pass("PCI MMIO window covered");
    } else {
        test_fail("PCI MMIO window covered", "not covered");
    }
    if kernel_device_window_pte(ECAM_PAGE).is_some() {
        test_pass("ECAM window covered");
    } else {
        test_fail("ECAM window covered", "not covered");
    }

    // Ordinary user VAs must NOT be covered (they clear to 0 as before).
    if kernel_device_window_pte(0x5000_0000).is_none()
        && kernel_device_window_pte(0x7000_0000).is_none()
    {
        test_pass("ordinary user VAs uncovered");
    } else {
        test_fail("ordinary user VAs uncovered", "false coverage");
    }

    // Window-interior pages and (just past) window-end pages.
    if kernel_device_window_pte(INTERIOR_A).is_some()
        && kernel_device_window_pte(INTERIOR_B).is_some()
    {
        test_pass("window interior pages covered");
    } else {
        test_fail("window interior pages covered", "interior gap");
    }
}

/// Megapage demotion: a 4K map into a region covered by a cloned device
/// megapage (every user root inherits the PLIC 2MB leaf at 0x0c000000)
/// must demote the leaf to a full L0 table reproducing the megapage
/// translation, then install the caller's page over exactly its slot.
/// Pre-fix this map was silently refused (gnome-shell SIGSEGV family).
pub fn test_megapage_demotion() {
    test_group_start("megapage demotion (4K map over 2MB leaf)");

    // aarch64 stage-2: create_user_address_space is the mm lane's open
    // todo (arch/aarch64/mm/mm_ops.rs — panics "not yet implemented"),
    // so this probe cannot run there yet. The graceful-None skip below
    // covers the riscv64 "no allocator" case.
    #[cfg(feature = "aarch64")]
    {
        test_skip(
            "megapage demotion",
            "aarch64: create_user_address_space pending (mm lane stage-2 todo)",
        );
        return;
    }

    use crate::arch::mm::mm_ops::{
        create_user_address_space, PageTableWalker,
    };
    use crate::arch::mm::mmu_init::{free_user_page_tables, map_page};
    use crate::arch::mm::memory_layout::{PhysAddr, VirtAddr};
    use crate::arch::mm::PageTableEntry;
    use crate::mm::page_alloc::get_zeroed_page;
    use crate::mm::zone::GfpFlags;

    // The unit-test harness runs before the kernel root exists in some
    // configurations; skip rather than fail when cloning is unavailable.
    let root_ppn = match create_user_address_space() {
        Some(r) => r,
        None => {
            test_skip("megapage demotion", "no address-space allocator in test context");
            return;
        }
    };

    unsafe {
        // Pre-state: 0x0c001000 resolves through the megapage to the
        // identity device frame, kernel-only.
        let before = PageTableWalker::walk(root_ppn, 0x0c00_1000);
        let had_leaf = before
            .map(|(ppn, bits)| {
                ppn == (0x0c00_1000 >> 12) && bits & PageTableEntry::U == 0
            })
            .unwrap_or(false);
        if !had_leaf {
            test_skip("megapage demotion", "device megapage not present in clone");
            free_user_page_tables(&crate::mm::mm_struct::FRESH_TREE_PTE_LOCK, root_ppn);
            return;
        }

        // The fix under test: map a user page straight into the megapage.
        let ram = get_zeroed_page(GfpFlags::GFP_KERNEL);
        if ram == 0 {
            test_skip("megapage demotion", "no RAM page for the probe");
            free_user_page_tables(&crate::mm::mm_struct::FRESH_TREE_PTE_LOCK, root_ppn);
            return;
        }
        let user_flags = PageTableEntry::V
            | PageTableEntry::R
            | PageTableEntry::W
            | PageTableEntry::U
            | PageTableEntry::A
            | PageTableEntry::D;
        map_page(
            root_ppn,
            VirtAddr::new(0x0c00_1000),
            PhysAddr::new(ram as u64),
            user_flags,
        );

        // The caller's page: its own frame, user-accessible.
        match PageTableWalker::walk(root_ppn, 0x0c00_1000) {
            Some((ppn, bits)) => {
                if ppn == (ram as u64 >> 12) && bits & PageTableEntry::U != 0 {
                    test_pass("4K map installed over megapage");
                } else {
                    test_fail("4K map installed over megapage", "wrong PTE after map");
                }
            }
            None => test_fail("4K map installed over megapage", "no PTE after map"),
        }

        // The neighbours: still translate to the device identity frames,
        // kernel-only — the demotion reproduced the megapage exactly.
        let mut ok = true;
        for probe in [0x0c00_0000u64, 0x0c00_2000, 0x0c00_3000] {
            match PageTableWalker::walk(root_ppn, probe) {
                Some((ppn, bits)) => {
                    if ppn != (probe >> 12) || bits & PageTableEntry::U != 0 {
                        ok = false;
                    }
                }
                None => ok = false,
            }
        }
        if ok {
            test_pass("demotion preserved neighbouring device pages");
        } else {
            test_fail("demotion preserved neighbouring device pages", "neighbour PTE wrong");
        }

        // A second 4K map in the same 2MB region must take the now-
        // ordinary L0 path (no double demotion, no refusal).
        let ram2 = get_zeroed_page(GfpFlags::GFP_KERNEL);
        if ram2 != 0 {
            map_page(
                root_ppn,
                VirtAddr::new(0x0c00_2000),
                PhysAddr::new(ram2 as u64),
                user_flags,
            );
            match PageTableWalker::walk(root_ppn, 0x0c00_2000) {
                Some((ppn, bits)) => {
                    if ppn == (ram2 as u64 >> 12) && bits & PageTableEntry::U != 0 {
                        test_pass("second 4K map in demoted region");
                    } else {
                        test_fail("second 4K map in demoted region", "wrong PTE");
                    }
                }
                None => test_fail("second 4K map in demoted region", "no PTE"),
            }
        }

        // Teardown of a root with a demoted table: the device (U=0)
        // entries must be skipped, the table frames freed — no crash,
        // no put_page of MMIO frames.
        free_user_page_tables(&crate::mm::mm_struct::FRESH_TREE_PTE_LOCK, root_ppn);
        test_pass("teardown of demoted root survived");
    }
}

pub fn test_reparent_safety() {
    test_plausible_task_ptr();
    test_move_child();
    test_claim_child();
    test_device_window_pte();
    test_megapage_demotion();
    let _ = println!(""); // keep the group separators tidy
}
