
// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]
// Sync-primitive protocol rows (review-s2-c): the preempt_count
// accounting of the SeqLock writer guards and the two-phase RwSpinlock
// write path. The seqlock row is the A/B for the try_write fix: the old
// body added PREEMPT_OFFSET (1) on acquire while the guard's Drop
// subtracted SOFTIRQ_OFFSET (256), leaving preempt_count at -255 after a
// successful try_write — this row reads the count INSIDE the section
// (must be exactly base+SOFTIRQ_OFFSET: BH disable also disables
// preemption) and AFTER the drop (must be back to the entry value).
use crate::sync::rwlock::RwSpinlock;
use crate::sync::seqlock::SeqLock;
use super::{test_fail, test_group_start, test_pass};

pub fn test_sync_locks() {
    test_group_start("sync_locks");

    let base = crate::interrupt::preempt::preempt_count();

    // ---- SeqLock::write() bracket (the reference shape) ----
    {
        let lock: SeqLock<u64> = SeqLock::new(11);
        let g = lock.write();
        let in_section = crate::interrupt::preempt::preempt_count();
        drop(g);
        let after = crate::interrupt::preempt::preempt_count();
        if in_section != base + crate::interrupt::preempt::SOFTIRQ_OFFSET {
            test_fail(
                "seqlock write bracket",
                "count inside write() != base+SOFTIRQ_OFFSET",
            );
            return;
        }
        if after != base {
            test_fail("seqlock write bracket", "count not restored after Drop");
            return;
        }
        test_pass("seqlock write bracket");
    }

    // ---- SeqLock::try_write() bracket (the fixed shape; was -255 after) ----
    {
        let lock: SeqLock<u64> = SeqLock::new(22);
        let g = match lock.try_write() {
            Some(g) => g,
            None => {
                test_fail("seqlock try_write bracket", "try_write failed on a free lock");
                return;
            }
        };
        let in_section = crate::interrupt::preempt::preempt_count();
        drop(g);
        let after = crate::interrupt::preempt::preempt_count();
        if in_section != base + crate::interrupt::preempt::SOFTIRQ_OFFSET {
            test_fail(
                "seqlock try_write bracket",
                "count inside try_write != base+SOFTIRQ_OFFSET",
            );
            return;
        }
        if after != base {
            test_fail(
                "seqlock try_write bracket",
                "count not restored after Drop (the pre-fix shape left -255)",
            );
            return;
        }
        // And the refused arm must restore too.
        let lock2: SeqLock<u64> = SeqLock::new(33);
        let held = lock2.write();
        if lock2.try_write().is_some() {
            test_fail("seqlock try_write refused", "try_write barged past a writer");
            return;
        }
        drop(held);
        // After BOTH the refused try_write and the live write() guard
        // dropped: the count must be exactly back at base.
        if crate::interrupt::preempt::preempt_count() != base {
            test_fail(
                "seqlock try_write refused",
                "refused try_write left the count moved",
            );
            return;
        }
        test_pass("seqlock try_write bracket");
    }

    // ---- RwSpinlock two-phase write protocol mechanics ----
    {
        let rw: RwSpinlock<u64> = RwSpinlock::new(44);
        // A held reader must gate try_write.
        {
            let _r = rw.read();
            if rw.try_write().is_some() {
                test_fail("rwlock write protocol", "try_write barged past a reader");
                return;
            }
        }
        // Writer takes and releases through the two-phase path.
        {
            let mut w = rw.write();
            if rw.try_read().is_some() {
                test_fail("rwlock write protocol", "try_read entered under the writer");
                return;
            }
            *w = 45;
        }
        // Clean handoff after write_unlock: a reader enters, the value
        // survives, and a follow-up write still takes.
        if *rw.read() != 45 {
            test_fail("rwlock write protocol", "write data lost across unlock");
            return;
        }
        {
            let _w2 = rw.write();
        }
        if rw.is_locked() {
            test_fail("rwlock write protocol", "lock not free after write_unlock");
            return;
        }
        test_pass("rwlock write protocol");
    }

    // ---- Rwsem (sleepable, the mmap_lock shape) fast-path protocol ----
    // Single-CPU rows: the preempt bracket of the fast paths, count/owner
    // word shapes, reader sharing, and the trylock refusal rules. The
    // cross-CPU grant/handoff protocol is exercised by every mmap/fault
    // in the suite (vma_manager IS an Rwsem) and by the OH boots.
    {
        use crate::sync::rwsem::Rwsem;

        let sem: Rwsem<u64> = Rwsem::new("test-rwsem", 7);

        // Uncontended read: preempt balanced, and a second reader may
        // join (readers share).
        {
            let r1 = sem.read();
            let in_section = crate::interrupt::preempt::preempt_count();
            let r2 = sem.read();
            if in_section != base {
                // The sleepable lock must NOT pin preemption while held
                // (only the fast-path RMW section is bracketed).
                test_fail(
                    "rwsem read protocol",
                    "held read guard pins preempt_count (must be preemptible)",
                );
                return;
            }
            if *r1 != 7 || *r2 != 7 {
                test_fail("rwsem read protocol", "data not visible to readers");
                return;
            }
            drop(r2);
            drop(r1);
            if crate::interrupt::preempt::preempt_count() != base {
                test_fail("rwsem read protocol", "preempt_count not restored after up_read");
                return;
            }
            if sem.is_locked() {
                test_fail("rwsem read protocol", "count word nonzero after last up_read");
                return;
            }
        }

        // Writer: try_write must refuse under a live reader; a real write
        // excludes try_read and restores the count word on drop.
        {
            let _r = sem.read();
            if sem.try_write().is_some() {
                test_fail("rwsem write protocol", "try_write barged past a live reader");
                return;
            }
            drop(_r);
            let mut w = sem.write();
            if sem.try_read().is_some() {
                test_fail("rwsem write protocol", "try_read entered under the writer");
                return;
            }
            *w = 8;
            drop(w);
            if sem.is_locked() {
                test_fail("rwsem write protocol", "count word nonzero after up_write");
                return;
            }
            if *sem.read() != 8 {
                test_fail("rwsem write protocol", "write data lost across unlock");
                return;
            }
        }

        // try_read under a live reader: legal (readers share). Queued-
        // waiter refusal (READ_FAILED_MASK) needs real contention —
        // exercised by the SMP suite and the OH boots.
        {
            let _r = sem.read();
            if sem.try_read().is_none() {
                test_fail("rwsem try_read protocol", "try_read refused under a bare reader");
                return;
            }
        }
        if sem.is_locked() {
            test_fail("rwsem try_read protocol", "count word nonzero after drops");
            return;
        }
        test_pass("rwsem fast-path protocol");
    }
}
