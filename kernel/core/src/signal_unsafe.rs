//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! SIGNAL EDGES (the signal crate-flip): the signal-family functions
//! whose bodies contain audited unsafe and therefore stay framework-side
//! when the POLICY moved to the signal service crate. Bodies are
//! verbatim relocations from kernel/core/src/signal.rs (the unsafe-edges
//! convention: unsafe moves with its function, policy moves without).
//!
//! Current residents: abort_reason_dump (the oh-svc SIGABRT forensics —
//! address-space and dentry raw-pointer reads).

/// oh-svc abort-reason dump: name the aborting binary/library and the
/// return chain at the moment of a self-raised SIGABRT. See the tap in
/// send_signal_from_user_tkill — runs on the raising task, in its
/// syscall context, so user registers and user memory are live.
pub fn abort_reason_dump(leader_tid: u32) {


    let task = match crate::sched::current() {
        Some(t) => t,
        None => return,
    };
    let (comm, tgid) = (task.comm(), task.tgid());
    let comm = core::str::from_utf8(comm).unwrap_or("").trim_end_matches('\0');

    let (rip, rsp) = {
        #[cfg(feature = "x86_64")]
        {
            crate::arch::trap::current_task_pt_regs()
                .map(|r| (r.rip, r.rsp))
                .unwrap_or((0, 0))
        }
        #[cfg(not(feature = "x86_64"))]
        {
            (0u64, 0u64)
        }
    };
    crate::pr_warn!(
        "ABORT-DUMP tgid={} tid={} comm='{}' rip={:#x} rsp={:#x}",
        tgid,
        leader_tid,
        comm,
        rip,
        rsp
    );

    let Some(aspace) = (unsafe { task.address_space_arc() }) else {
        crate::pr_warn!("ABORT-DUMP: no address space");
        return;
    };

    // Pass 1 (vma_read held, no name resolution): capture every
    // executable VMA's range. render_service maps hundreds of libraries;
    // the pass must cover ALL of them or the abort chain's return
    // addresses fall outside the snapshot.
    let mut ranges: [(u64, u64, usize); 1024] = [(0, 0, 0); 1024];
    let mut n = 0usize;
    {
        let vma_mgr = aspace.vma_read();
        for vma in vma_mgr.iter() {
            if n >= 1024 {
                break;
            }
            if !vma.flags().is_executable() {
                continue;
            }
            ranges[n] = (vma.start().as_usize() as u64, vma.end().as_usize() as u64, vma.offset());
            n += 1;
        }
    }

    // Pass 2: classify the user rip and the stack words; resolve the
    // owning library's name lazily (only for values that hit a range).
    let label = |addr: u64| -> alloc::string::String {
        for i in 0..n {
            let (s, e, _off) = ranges[i];
            if addr >= s && addr < e {
                let name = aspace.get_vma_file(s as usize).and_then(|file| {
                    // SAFETY: read-only dentry access on the pinned file.
                    let dentry = unsafe { &*file.dentry.get() };
                    dentry.as_ref().map(|d| d.name.lock().clone())
                });
                return match name {
                    Some(name) => alloc::format!("{}+{:#x}", name, addr - s),
                    None => alloc::format!("exec+{:#x}", addr - s),
                };
            }
        }
        alloc::format!("{:#x}", addr)
    };
    crate::pr_warn!("ABORT-DUMP rip-in {}", label(rip));
    crate::pr_warn!("ABORT-MAPS n={} exec-vmas", n);

    // Return-chain scan: 64 qwords from user rsp upward; values landing
    // in an exec VMA print as lib+offset, everything else raw (the
    // abort chain can sit a few hundred bytes above rsp when CFI shims
    // pad the frames).
    let mut line = alloc::string::String::new();
    for i in 0..64u64 {
        let mut w = [0u8; 8];
        if crate::arch::uaccess_safe::copy_in(&mut w, (rsp + i * 8) as usize) != 0 {
            continue;
        }
        let v = u64::from_le_bytes(w);
        if v == 0 {
            continue;
        }
        let inside = ranges[..n].iter().any(|(s, e, _)| v >= *s && v < *e);
        if inside {
            line.push_str(&alloc::format!(" [{}]{}", i, label(v)));
        } else {
            line.push_str(&alloc::format!(" [{}]{:#x}", i, v));
        }
    }
    crate::pr_warn!("ABORT-STK{}", line);
}
