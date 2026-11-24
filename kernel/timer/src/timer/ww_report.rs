//! The WW census heartbeat REPORT half (moved from
//! kernel/src/timer.rs at the cold-four policy flip — sole copy here;
//! zero behavior change). The markers and their statics stay
//! framework-side (ww_note_tick_arrival / ww_note_rearm / the fire
//! counters — written on the tick/fire paths); this file only READS
//! them and formats. Reached through the arm-gated
//! timer_hooks::ww_heartbeat row (EVENT_WWCENSUS gate at the call
//! site — disarmed boots never load the table).

use crate::timer::{
    get_jiffies, wheel_census, ww_task_on_cpu, HRES_NEXT, NEXT_TIMER_ID,
    WW_FIRE_LATE_MAX_MS, WW_FIRE_MAX_US, WW_FIRE_N, WW_PIDMISS_N,
    WW_FIRE_SUM_TICKS, WW_REARM_N, WW_TICK_IRQ,
    FWT_PID, FWT_OK, FWT_ONRQ, FWT_KEY, FWT_ST, FWT_JIF,
};
use core::sync::atomic::{AtomicU64, Ordering};

/// Print one WW heartbeat line (called from the softirq, printk-safe
/// context — the claim-inject probe above prints from here too).
/// v2: adds the softirq-storm counters (per-vector dispatch deltas,
/// softirq busy ticks), timer-IRQ rate, scheduler slots (per-CPU current
/// pid/state/preempt_count, idle + need_resched masks, queue depths)
/// and the wheel census (len, overdue backlog) — everything the wt1/wt2
/// collapse discrimination needs on one line per second.
pub fn ww_heartbeat(now_time: u64) {
    use crate::interrupt::softirq::{WW_SOFTIRQ_RUNS, WW_SOFTIRQ_TICKS, WW_VEC_RUN};
    static LAST_VEC: [AtomicU64; crate::interrupt::softirq::NR_SOFTIRQS] =
        [const { AtomicU64::new(0) }; crate::interrupt::softirq::NR_SOFTIRQS];
    static LAST_RUNS: AtomicU64 = AtomicU64::new(0);
    static LAST_TICKS: AtomicU64 = AtomicU64::new(0);
    static LAST_IRQ: [AtomicU64; crate::config::MAX_CPUS] =
        [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

    let fire_n = WW_FIRE_N.load(Ordering::Relaxed);
    let late_max = WW_FIRE_LATE_MAX_MS.load(Ordering::Relaxed);
    let pidmiss = WW_PIDMISS_N.load(Ordering::Relaxed);

    let runs_d = WW_SOFTIRQ_RUNS.load(Ordering::Relaxed)
        .saturating_sub(LAST_RUNS.load(Ordering::Relaxed));
    // Divide first: raw tick deltas can approach u64::MAX (e.g. HRES_NEXT
    // sentinel) and a *1000 would overflow in the debug build.
    let ticks_per_ms = crate::config::TIMER_CLOCK_FREQ_HZ / 1000;
    let busy_ms = WW_SOFTIRQ_TICKS
        .load(Ordering::Relaxed)
        .saturating_sub(LAST_TICKS.load(Ordering::Relaxed))
        / ticks_per_ms;
    let mut vecs = [0u64; crate::interrupt::softirq::NR_SOFTIRQS];
    for v in 0..crate::interrupt::softirq::NR_SOFTIRQS {
        vecs[v] = WW_VEC_RUN[v].load(Ordering::Relaxed) as u64
            - LAST_VEC[v].load(Ordering::Relaxed);
        LAST_VEC[v].store(WW_VEC_RUN[v].load(Ordering::Relaxed) as u64, Ordering::Relaxed);
    }
    LAST_RUNS.store(WW_SOFTIRQ_RUNS.load(Ordering::Relaxed), Ordering::Relaxed);
    LAST_TICKS.store(WW_SOFTIRQ_TICKS.load(Ordering::Relaxed), Ordering::Relaxed);

    let mut irq_d = [0u64; crate::config::MAX_CPUS];
    for c in 0..crate::config::MAX_CPUS {
        let cur = WW_TICK_IRQ[c].load(Ordering::Relaxed);
        irq_d[c] = cur.saturating_sub(LAST_IRQ[c].load(Ordering::Relaxed));
        LAST_IRQ[c].store(cur, Ordering::Relaxed);
    }

    // s15 (A2 idle-mark/WFI hunt): idle-loop phase stamps + tp-vs-slot
    // mismatch deltas. phase codes: 0=not-idle-loop 1=entering-schedule
    // 2=marked+recheck 3=pre-WFI 4=inside-WFI 5=post-WFI-accounting;
    // slotmm = per-second delta of tick-entry samples where the hardware
    // tp register disagreed with cpu_state.current (lying slot).
    static LAST_MM: [AtomicU64; crate::config::MAX_CPUS] =
        [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];
    let mm = crate::sched::sched::ww_slot_mismatch_counters();
    let mut mm_d = [0u64; 2];
    for c in 0..2 {
        mm_d[c] = mm[c].saturating_sub(LAST_MM[c].load(Ordering::Relaxed));
        LAST_MM[c].store(mm[c], Ordering::Relaxed);
    }
    let phase0 = crate::sched::ww_idle_phase(0);
    let phase1 = crate::sched::ww_idle_phase(1);

    // Wheel census (short lock; heartbeat is 1 Hz and holds no timer
    // locks at this point). Wheel-side: len + overdue hres backlog.
    let (wheel_len, overdue) = crate::timer::wheel_census(now_time);

    let sched = crate::sched::sched::ww_sched_snapshot();

    // nanosleep-loss hunt: arm/fire accounting — NEXT_TIMER_ID-1 is the
    // total arms ever; fires + live entries + cancels must cover it. A
    // gap = entries vanishing unfired.
    {
        use crate::debug::taskdump::putc_brief as putchar;
        fn pdec9(mut v: u64) {
            use crate::debug::taskdump::putc_brief as p;
            let mut buf = [0u8; 20];
            let mut n = 0;
            if v == 0 { p(b'0'); return; }
            while v > 0 && n < buf.len() { buf[n] = b'0' + (v % 10) as u8; v /= 10; n += 1; }
            for k in (0..n).rev() { p(buf[k]); }
        }
        for &b in b"NSLPARK rescued=" { putchar(b); }
        pdec9(crate::sched::sched::NSLPARK_RESCUED.load(Ordering::Relaxed));
        putchar(b'\n');
        for &b in b"ARMS id=" { putchar(b); }
        pdec9(NEXT_TIMER_ID.load(Ordering::Relaxed).saturating_sub(1));
        for &b in b" fires=" { putchar(b); }
        pdec9(WW_FIRE_N.load(Ordering::Relaxed));
        for &b in b" live=" { putchar(b); }
        {
            pdec9(crate::timer::wheel_census(now_time).0 as u64);
        }
        putchar(b'\n');
    }
    // nanosleep-loss lane (hunt): per-task pick-phase census — every
    // heartbeat, one line per task: pid, last pick-phase (1 walk-unlink,
    // 2 mark_picked, 3 slot-published, 4 repick-return, 5 wrapper-enter,
    // 6 affirm-ok, 7 affirm-fail, 8 early-abort, 9 asm-abort), phase age
    // (jiffies), ownership qword, pk, linked, state. A frozen task names
    // its terminal phase directly.
    {
        use crate::debug::taskdump::putc_brief as putchar;
        fn pdec(mut v: u64) {
            use crate::debug::taskdump::putc_brief as p;
            let mut buf = [0u8; 20];
            let mut n = 0;
            if v == 0 { p(b'0'); return; }
            while v > 0 && n < buf.len() { buf[n] = b'0' + (v % 10) as u8; v /= 10; n += 1; }
            for k in (0..n).rev() { p(buf[k]); }
        }
        static PTBUDGET: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        // nanosleep-loss: CPU-0 only — the direct-console census writes
        // garble when both CPUs print concurrently.
        if crate::arch::cpu_id() as usize == 0
            && PTBUDGET.fetch_add(1, Ordering::Relaxed) < 60 {
            putchar(b'\n');
            crate::process::pid_hash::pid_hash_for_each_task_try(|task| {
                // The walk hands &Task under the bucket lock — plain reads.
                // Helper APIs taking raw identity get the safe &-to-raw cast.
                let task_raw = task as *const crate::process::task::Task
                    as *mut crate::process::task::Task;
                {
                let pid = task.pid();
                if pid == 0 || pid == crate::sched::sched::TASK_POISON { return; }
                let q = crate::timer::ww_task_on_cpu(task);
                let st = task.state().bits();
                let linked = match task.policy() {
                    crate::process::task::SchedPolicy::Fifo
                    | crate::process::task::SchedPolicy::Rr => false,
                    _ => task.sched_entity().timeline_key().is_some(),
                };
                let (ptc, ptj) =
                    match crate::sched::sched::pick_trace_peek(task_raw) {
                        Some(x) => x,
                        None => (0, 0),
                    };
                for &b in b"PTC pid=" { putchar(b); }
                pdec(pid as u64);
                for &b in b" ph=" { putchar(b); }
                pdec(ptc as u64);
                for &b in b" ago=" { putchar(b); }
                pdec(ptj);
                for &b in b" q=0x" { putchar(b); }
                pdec(q);
                for &b in b" pk=" { putchar(b); }
                pdec(task.pick_state() as u64);
                for &b in b" lnk=" { putchar(b); }
                pdec(linked as u64);
                let tid = task.timed_sleep_timer_id.load(Ordering::Relaxed);
                let tdl = task.timed_sleep_deadline.load(Ordering::Relaxed);
                let tpend = crate::timer::timer_pending(tid);
                for &b in b" tid=" { putchar(b); }
                pdec(tid);
                for &b in b" tp=" { putchar(b); }
                pdec(tpend as u64);
                for &b in b" tdl-now=" { putchar(b); }
                let nowt = crate::timer::read_time();
                if tdl > nowt {
                    pdec((tdl - nowt) * 1000 / crate::config::TIMER_CLOCK_FREQ_HZ as u64);
                } else {
                    putchar(b'-');
                    pdec((nowt - tdl) * 1000 / crate::config::TIMER_CLOCK_FREQ_HZ as u64);
                }
                for &b in b" st=0x" { putchar(b); }
                pdec(st as u64);
                for &b in b" itr=" { putchar(b); }
                pdec(crate::timer::nsleep_iter_of(task_raw));
                // last fire-record for this pid (per-pid table)
                for k in 0..32 {
                    if FWT_PID[k].load(Ordering::Relaxed) == pid {
                        for &b in b" fwj=" { putchar(b); }
                        pdec(crate::timer::get_jiffies().saturating_sub(
                            FWT_JIF[k].load(Ordering::Relaxed)));
                        for &b in b" fw=(ok:" { putchar(b); }
                        pdec(FWT_OK[k].load(Ordering::Relaxed) as u64);
                        for &b in b",onrq:" { putchar(b); }
                        pdec(FWT_ONRQ[k].load(Ordering::Relaxed) as u64);
                        for &b in b",key:" { putchar(b); }
                        pdec(FWT_KEY[k].load(Ordering::Relaxed) as u64);
                        for &b in b",st:0x" { putchar(b); }
                        pdec(FWT_ST[k].load(Ordering::Relaxed) as u64);
                        putchar(b')');
                        break;
                    }
                }
                putchar(b'\n');
                }
            });
        }
    }
    // s16 (WW5, A2 census): pick-walk and heal deltas plus cumulative
    // sub-gate strand counts — the counter-based adjudication of the
    // msg-region strand family (see fair.rs WW_* counters). OWN pr_info:
    // the WW..WW4 block above is already ~250 bytes and printk truncates
    // at RECORD_TEXT_SIZE (256) — appending here silently cut the tail.
    static LAST_WALKS: AtomicU64 = AtomicU64::new(0);
    static LAST_OH: AtomicU64 = AtomicU64::new(0);
    static LAST_CH: AtomicU64 = AtomicU64::new(0);
    let walks_total = crate::sched::ww_pick_walks();
    let walks_d = walks_total.saturating_sub(LAST_WALKS.load(Ordering::Relaxed));
    LAST_WALKS.store(walks_total, Ordering::Relaxed);
    let (oh, ch) = crate::sched::sched_hooks::ww_heals();
    let oh_d = oh.saturating_sub(LAST_OH.load(Ordering::Relaxed));
    let ch_d = ch.saturating_sub(LAST_CH.load(Ordering::Relaxed));
    LAST_OH.store(oh, Ordering::Relaxed);
    LAST_CH.store(ch, Ordering::Relaxed);
    let (s250, s500, s750) = crate::sched::sched_hooks::ww_subgate_counts();
    crate::pr_info!(
        "WW5: walks={} oncpu_heal={} claim_heal={} s250={} s500={} s750={}",
        walks_d, oh_d, ch_d, s250, s500, s750
    );
    // s16: WW4 (phase/slotmm) in its own pr_info too — as the TAIL of
    // the big block it was the part most often garbled by the per-CPU
    // wakewatch heartbeat racing the UART, and slotmm is the family-B
    // standing watch (it must stay parseable).
    crate::pr_info!(
        "WW4: phase=[{},{}] slotmm=[{},{}]",
        phase0,
        phase1,
        mm_d[0],
        mm_d[1]
    );

    // rt-r4 (R2 residual attribution): the AVG side of the CYC brackets —
    // cumulative hres fire-lateness mean/max plus the RT-class wake→pick
    // mean/max. With a cyclictest FIFO run, cyclictest_avg ≈ fire_avg +
    // rt_pick_avg + switch/user overhead; the residual attribution names
    // the top term from these two lines. Divide-first tick→µs (the WW
    // overflow discipline: sums can grow unbounded across a long boot).
    {
        let ticks_per_us = (crate::config::TIMER_CLOCK_FREQ_HZ / 1_000_000).max(1);
        let f_n = WW_FIRE_N.load(Ordering::Relaxed);
        let f_sum_us = WW_FIRE_SUM_TICKS.load(Ordering::Relaxed) / ticks_per_us;
        let f_avg = if f_n > 0 { f_sum_us / f_n } else { 0 };
        let (rt_n, rt_avg, rt_max) = crate::sched::sched_hooks::ww_rt_pick_stats();
        crate::pr_info!(
            "WW6: fire_n={} fire_avg={}us fire_max={}us rt_pick_n={} rt_pick_avg={}us rt_pick_max={}us",
            f_n,
            f_avg,
            WW_FIRE_MAX_US.load(Ordering::Relaxed),
            rt_n,
            rt_avg,
            rt_max
        );
    }

    crate::pr_info!(
        "WW: fires={} late_max={}ms pidmiss={} rearm={} wheel={}/{} hres_next={}ms\nWW2: si_runs={} si_busy={}ms ticks=[{},{}] vec=[{},{},{},{},{},{},{},{},{},{}]\nWW3: grq={} cfs={} idle={:#x} nr={} cpu0=(pid={} st={:#x} pc={}) cpu1=(pid={} st={:#x} pc={})",
        fire_n,
        late_max,
        pidmiss,
        WW_REARM_N.load(Ordering::Relaxed),
        overdue,
        wheel_len,
        HRES_NEXT.load(Ordering::Acquire).saturating_sub(now_time) / ticks_per_ms,
        runs_d,
        busy_ms,
        irq_d[0],
        irq_d[1],
        vecs[0], vecs[1], vecs[2], vecs[3], vecs[4],
        vecs[5], vecs[6], vecs[7], vecs[8], vecs[9],
        sched[0], sched[1], sched[2], sched[3],
        sched[4], sched[5], sched[6],
        sched[7], sched[8], sched[9]
    );
    crate::sched::sched_hooks::heartbeat();
    // sc158 (Session R): one-shot park dump for wedged foundation/sa_main
    // registrar threads - see wakewatch::stallwatch_tick.
    crate::sched::sched_hooks::stallwatch_tick();
}

/// The DEL_DM diagnostic ring's reader (moved from kernel/src/timer.rs
/// — sole copy; the DEL_* marker statics stay framework-side, written
/// by the del-timer path). Caller: process's exit teardown.
pub fn del_lookup(id: u64) -> Option<(u64, u64)> {
    use crate::timer::{DEL_ID, DEL_JIF, DEL_RA};
    for k in 0..64 {
        if DEL_ID[k].load(Ordering::Relaxed) == id {
            return Some((
                DEL_RA[k].load(Ordering::Relaxed),
                get_jiffies()
                    .saturating_sub(DEL_JIF[k].load(Ordering::Relaxed)),
            ));
        }
    }
    None
}

/// The nanosleep loop-iteration marker (moved from kernel/src/timer.rs
/// — sole copy; the NSL_* statics stay framework-side with the reader
/// the WW report uses). Caller: syscall's nanosleep_impl.
#[inline]
pub fn nsleep_iter_bump() {
    use crate::timer::{NSL_ITER, NSL_TASK};
    let Some(t) = crate::sched::current() else { return };
    let key = t as *mut crate::process::Task as usize;
    for k in 0..16 {
        if NSL_TASK[k].load(Ordering::Relaxed) == key {
            NSL_ITER[k].fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
    for k in 0..16 {
        if NSL_TASK[k]
            .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            NSL_ITER[k].fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
}
