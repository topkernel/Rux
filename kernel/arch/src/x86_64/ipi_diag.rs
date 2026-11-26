//! The x86_64 remote-BT requester half (arch-cold lane): moved from
//! kernel/core/src/arch/x86_64/ipi.rs. The IRQ-work capture halves
//! (snapshot_self, remote_bt_self — the IPI handler bodies with their
//! raw frame-pointer walks) stayed framework-side writing the pub
//! BT_*/SNAP_* statics; this is the requester that arms the sequence,
//! polls, and PRINTS the captured context. Callers (deadlock warn,
//! softlockup, the monopolist hunt) reach it through the framework's
//! historical-name wrapper on the ArchDiagHooks row.

use core::sync::atomic::Ordering;

use crate::arch::x86_64::ipi::{
    send_ipi_type, IpiType, BT_DONE, BT_FRAMES, BT_INFLIGHT, BT_NFRAMES, BT_REQ,
    BT_RBP, BT_RIP, BT_RSP, REMOTE_BT_MAX_FRAMES,
};

pub fn remote_backtrace(target: usize, why: &str) -> bool {
    if target >= crate::config::MAX_CPUS || !crate::arch::smp::cpu_started(target) {
        return false;
    }
    let me = crate::arch::cpu_id() as usize;
    if target == me {
        return false;
    }
    // One hunter at a time (see BT_INFLIGHT comment).
    if BT_INFLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    let req = BT_REQ[target].fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    send_ipi_type(target, IpiType::IrqWork);
    // Bounded poll (~200M relaxed loads ≈ the DEADLOCK_WARN spin-budget
    // class — well under a second on KVM).
    let mut spins: u32 = 0;
    while BT_DONE[target].load(Ordering::Acquire) != req {
        spins = spins.wrapping_add(1);
        if spins > 200_000_000 {
            let mut w = crate::debug::backtrace::ConsoleWriter::new();
            let _ = core::fmt::Write::write_fmt(
                &mut w,
                format_args!("REMOTE-BT[{}]: cpu={} TIMEOUT (not servicing IPIs)\n", why, target),
            );
            BT_INFLIGHT.store(false, Ordering::Release);
            return false;
        }
        core::hint::spin_loop();
    }
    let rip = BT_RIP[target].load(Ordering::Relaxed);
    let rsp = BT_RSP[target].load(Ordering::Relaxed);
    let rbp = BT_RBP[target].load(Ordering::Relaxed);
    let n = BT_NFRAMES[target].load(Ordering::Relaxed) as usize;
    let mut w = crate::debug::backtrace::ConsoleWriter::new();
    // Target's current task pid/comm (best-effort; comm is fixed-size
    // bytes, no allocation).
    let cur = crate::sched::hotpath::cpu_state(target).current;
    if cur.is_null() {
        let _ = core::fmt::Write::write_fmt(
            &mut w,
            format_args!("REMOTE-BT[{}] cpu={}: (no current task) rip={:#x} rsp={:#x}\n", why, target, rip, rsp),
        );
    } else {
        let (pid, comm) = crate::arch::x86_64::r#unsafe::diag_task_pid_comm(cur);
        let mut cbuf = [0u8; 17];
        let mut cl = 0;
        for &b in comm.iter() {
            if b == 0 || cl == 16 {
                break;
            }
            if (0x20..0x7f).contains(&b) {
                cbuf[cl] = b;
                cl += 1;
            }
        }
        let cname = core::str::from_utf8(&cbuf[..cl]).unwrap_or("?");
        let _ = core::fmt::Write::write_fmt(
            &mut w,
            format_args!("REMOTE-BT[{}] cpu={} task={}({}) rip={:#x} rsp={:#x} rbp={:#x}\n",
                why, target, pid, cname, rip, rsp, rbp),
        );
    }
    if rbp == 0 {
        let _ = core::fmt::Write::write_str(&mut w, "REMOTE-BT: target was in USER mode\n");
    }
    let base = target * REMOTE_BT_MAX_FRAMES;
    for i in 0..n.min(REMOTE_BT_MAX_FRAMES) {
        let a = BT_FRAMES[base + i].load(Ordering::Relaxed);
        let _ = core::fmt::Write::write_fmt(
            &mut w,
            format_args!("REMOTE-BT  #{:02} {:#x}\n", i, a),
        );
    }
    BT_INFLIGHT.store(false, Ordering::Release);
    true
}

