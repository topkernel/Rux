
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
}
