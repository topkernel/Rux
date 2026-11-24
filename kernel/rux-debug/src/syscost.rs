//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The syscost REPORT half (moved from kernel/src/debug/syscost.rs,
//! crate-flip rung rux-debug): the 60 s cadence dump — tick modes, errno
//! census, flood attribution, IF=0 zone decomposition, REST call-site
//! table, UART cost, printk volume, recv census, the syscall table and
//! the kernel-rip histogram. The record markers, their statics and the
//! cadence gate (tick) stay framework-side; this module READS the
//! statics through their pub peek surface. Body moved verbatim — the
//! output format is load-bearing (scripts/perf-gate.sh and
//! scripts/test/pa_parse.py parse these lines by text).

use core::sync::atomic::Ordering;

use alloc::string::ToString;

use crate::debug::syscost::*;
use crate::debug::syscost::zone_cur;

/// Print the full report (also usable from a debug trigger).
pub fn dump() {
    // Nominal domain: 10 ticks per microsecond by definition.
    const US: u64 = 10;
    let secs = crate::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;

    // 39th: AF_UNIX hot-path phase anatomy rides the same dump cadence
    // (no-op unless the unixrd=1 cmdline param is armed).
    crate::debug::unixrd::dump();
    // hilog-flood lane: OH log-fabric per-caller census (no-op unless
    // the hilogcensus=1 cmdline param is armed).
    crate::debug::hilogcensus::dump();
    // rt-r4: per-caller binder WORLD section maxima (all WORLD sections
    // share one REST_SITES row — this splits the 6-8 ms class by caller).
    {
        let rep = crate::ipc::binder::world_section_report();
        if !rep.is_empty() {
            crate::pr_warn!("SYSCOST t={}s{}", secs, rep);
        }
    }

    // Tick modes: one line per dump.
    let mut line = alloc::string::String::from("SYSCOST t=");
    line.push_str(&secs.to_string());
    line.push_str("s ticks(u/k/i):");
    for c in 0..crate::config::MAX_CPUS {
        let u = TICK_USER[c].load(Ordering::Relaxed);
        let k = TICK_KERN[c].load(Ordering::Relaxed);
        let i = TICK_IDLE[c].load(Ordering::Relaxed);
        if u | k | i == 0 {
            continue;
        }
        line.push_str(&alloc::format!(" c{}={}/{}/{}", c, u, k, i));
    }
    crate::pr_warn!("{}", line);

    // Softirq vector dispatch counts (the existing WW_VEC_RUN / run /
    // busy-tick counters, printed cumulatively): names the vector behind
    // the __do_softirq page burn seen in the rip histogram.
    {
        use crate::interrupt::softirq::{WW_SOFTIRQ_RUNS, WW_SOFTIRQ_TICKS, WW_VEC_RUN};
        let mut line = alloc::string::String::new();
        for v in 0..crate::interrupt::softirq::NR_SOFTIRQS {
            let n = WW_VEC_RUN[v].load(Ordering::Relaxed) as u64;
            if n > 0 {
                line.push_str(&alloc::format!(" v{}={}", v, n));
            }
        }
        let busy_ms = WW_SOFTIRQ_TICKS.load(Ordering::Relaxed)
            / (crate::config::TIMER_CLOCK_FREQ_HZ / 1000);
        crate::pr_warn!(
            "SYSCOST t={}s softirq: runs={} busy={}ms vecs:{}",
            secs,
            WW_SOFTIRQ_RUNS.load(Ordering::Relaxed),
            busy_ms,
            line
        );
    }

    // Flood-nr per-comm attribution (which process drives the 10^5/s
    // calls; v0=Hi v1=Timer v2=NetTx v3=NetRx v4=Block v5=IrqPoll
    // v6=Tasklet v7=Sched v8=Hrtimer v9=Rcu for the vec line above).
    {
        use crate::printk::{KMSG_N_EAGAIN, KMSG_N_RECORD, KMSG_N_SLEEP, KMSG_FIRST_FLAGS};
        crate::pr_warn!(
            "SYSCOST t={}s kmsg: rec={} eagain={} sleep={} first_flags={:#x}",
            secs,
            KMSG_N_RECORD.load(Ordering::Relaxed),
            KMSG_N_EAGAIN.load(Ordering::Relaxed),
            KMSG_N_SLEEP.load(Ordering::Relaxed),
            KMSG_FIRST_FLAGS.load(Ordering::Relaxed)
        );
    }

    // launcher-errno lane: the per-(nr, errno) census, sorted by count.
    // First-seen boot-time attributes each cell to a boot phase; first
    // comm + first a0 name the caller and its argument.
    {
        use core::sync::atomic::Ordering::Relaxed;
        use alloc::vec::Vec;
        let mut rows: Vec<(u64, i64, u64, u64, [u8; 16], u64)> = Vec::new();
        for i in 0..ERR_CENSUS.len() {
            let n = ERR_CENSUS[i].n.load(Relaxed);
            if n == 0 {
                continue;
            }
            let mut comm = [0u8; 16];
            for j in 0..16 {
                comm[j] = ERR_CENSUS[i].comm[j].load(Relaxed);
            }
            rows.push((
                ERR_CENSUS[i].nr.load(Relaxed),
                ERR_CENSUS[i].err.load(Relaxed),
                n,
                ERR_CENSUS[i].first_ms.load(Relaxed),
                comm,
                ERR_CENSUS[i].a0.load(Relaxed),
            ));
        }
        rows.sort_by(|a, b| b.2.cmp(&a.2));
        for (nr, err, n, first_ms, comm, a0) in rows {
            let name = core::str::from_utf8(&comm)
                .unwrap_or("?")
                .split('\0')
                .next()
                .unwrap_or("?");
            crate::pr_warn!(
                "SYSCOST t={}s errno: nr={} err={} n={} first={}.{:03}s comm={} a0={:#x}",
                secs,
                nr,
                err,
                n,
                first_ms / 1000,
                first_ms % 1000,
                name,
                a0
            );
        }
        // Path strings (cells with a snapshot; separate pass so the main
        // line format stays diff-stable with earlier boots).
        for i in 0..ERR_CENSUS.len() {
            if ERR_CENSUS[i].n.load(Relaxed) == 0 {
                continue;
            }
            let mut path = [0u8; 64];
            for j in 0..64 {
                path[j] = ERR_CENSUS[i].path[j].load(Relaxed);
            }
            if path[0] == 0 {
                continue;
            }
            let text = core::str::from_utf8(&path)
                .unwrap_or("?")
                .split('\0')
                .next()
                .unwrap_or("?");
            crate::pr_warn!(
                "SYSCOST t={}s errno-path: nr={} err={} path={}",
                secs,
                ERR_CENSUS[i].nr.load(Relaxed),
                ERR_CENSUS[i].err.load(Relaxed),
                text
            );
        }
    }

    // 38th (u2v wedge hunt): the syscall ring's newest 64 entries join the
    // flood table's COMM attribution with the (pid, nr, a0=fd, ret) tuples.
    // The spin comm is known (hilogd.server); this names its fd and return
    // values at EVERY dump cadence — the hung-task one-shot replay caught
    // only the first (too-early) wedge phase (u2v4 @181s).
    if crate::trace::EVENT_SYSRING.armed() {
        use crate::syscall::dispatch::{SYSCALL_CURSOR, SYSCALL_RING};
        let cur = SYSCALL_CURSOR.load(Ordering::Relaxed);
        let len = SYSCALL_RING.len();
        for k in (0..64usize).rev() {
            let idx = (cur + len - 1 - k) % len;
            let e = &SYSCALL_RING[idx];
            let pid = e.pid.load(Ordering::Relaxed);
            let nr = e.nr.load(Ordering::Relaxed);
            let a0 = e.a0.load(Ordering::Relaxed);
            let ret = e.ret.load(Ordering::Relaxed) as i64;
            crate::pr_warn!("SYSCOST-RING {} {} {:#x} {}", pid, nr, a0, ret);
        }
    }
    for f in 0..FLOOD_NRS.len() {
        let mut line = alloc::string::String::new();
        for i in 0..16 {
            let n = FLOOD[f][i].n.load(Ordering::Relaxed);
            if n == 0 {
                continue;
            }
            let mut comm = [0u8; 16];
            for j in 0..16 {
                comm[j] = FLOOD[f][i].comm[j].load(Ordering::Relaxed);
            }
            let name = core::str::from_utf8(&comm)
                .unwrap_or("?")
                .split('\0')
                .next()
                .unwrap_or("?");
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&alloc::format!("{}={}", name, n));
        }
        if !line.is_empty() {
            crate::pr_warn!(
                "SYSCOST t={}s flood nr={}: {}",
                secs,
                FLOOD_NRS[f],
                line
            );
        }
    }

    // pa15/pa16 IF=0 zone decomposition (F1 irq-exit dispatch numbers;
    // raw TSC -> ms). Zones: rest / hardirq-frame / softirq-dispatch /
    // uart. v3: exact (edge-triggered, self-healing) — heal/miss are
    // the model-vs-reality disagreement meters and should read ~0.
    {
        let tsc_hz = crate::arch::cpu::get_counter_freq().max(1);
        let msd = tsc_hz / 1000;
        let usd = (tsc_hz / 1_000_000).max(1);
        for c in 0..crate::config::MAX_CPUS {
            let v = [
                IFZ.cum[c][0].load(Ordering::Relaxed),
                IFZ.cum[c][1].load(Ordering::Relaxed),
                IFZ.cum[c][2].load(Ordering::Relaxed),
                IFZ.cum[c][3].load(Ordering::Relaxed),
            ];
            let heal_n = IFZ.heal_n[c].load(Ordering::Relaxed);
            let miss_n = IFZ.miss_n[c].load(Ordering::Relaxed);
            if v.iter().all(|&x| x == 0) && heal_n == 0 && miss_n == 0 {
                continue;
            }
            let tot: u64 = v.iter().sum();
            let nseg = [
                IFZ.nseg[c][0].load(Ordering::Relaxed),
                IFZ.nseg[c][1].load(Ordering::Relaxed),
                IFZ.nseg[c][2].load(Ordering::Relaxed),
                IFZ.nseg[c][3].load(Ordering::Relaxed),
            ];
            let mx = [
                IFZ.seg_max[c][0].load(Ordering::Relaxed),
                IFZ.seg_max[c][1].load(Ordering::Relaxed),
                IFZ.seg_max[c][2].load(Ordering::Relaxed),
                IFZ.seg_max[c][3].load(Ordering::Relaxed),
            ];
            // Open-segment diagnostic (cross-CPU reads; KVM syncs TSC —
            // treat the age as indicative). A cpu parked by a KERNPANIC
            // shows a frozen, ever-aging open segment here.
            let mut open = alloc::string::String::new();
            if IFZ.if0[c].load(Ordering::Relaxed) != 0 {
                let zn = match zone_cur(c) {
                    ZONE_HARDIRQ => "hard",
                    ZONE_SOFTIRQ => "soft",
                    ZONE_UART => "uart",
                    _ => "rest",
                };
                open = alloc::format!(
                    " OPEN zone={} age={}ms",
                    zn,
                    (crate::arch::cpu::read_counter()
                        .saturating_sub(IFZ.t0[c].load(Ordering::Relaxed)))
                        / msd
                );
            }
            crate::pr_warn!(
                "SYSCOST t={}s if0 cpu={}: tot={}ms rest={}ms hard={}ms soft={}ms uart={}ms nseg={}/{}/{}/{} max={}us/{}us/{}us/{}us heal={}n/{}ms miss={}n{}",
                secs,
                c,
                tot / msd,
                v[0] / msd,
                v[1] / msd,
                v[2] / msd,
                v[3] / msd,
                nseg[0], nseg[1], nseg[2], nseg[3],
                // rt-r4: per-zone maxima in µs (the ms grid rounded the
                // 1 ms class to 0/1/2 — useless for the <1 ms gate).
                mx[0] / usd, mx[1] / usd, mx[2] / usd, mx[3] / usd,
                heal_n,
                IFZ.heal_cyc[c].load(Ordering::Relaxed) / msd,
                miss_n,
                open
            );
        }

        // REST open-site table: top 12 rows by charged cycles + overflow.
        // One row per file:line that opened an irqsave section; the fix
        // list for the F1 REST bulk.
        let mut top: [usize; 12] = [usize::MAX; 12];
        let mut top_cyc: [u64; 12] = [0; 12];
        for i in 0..REST_SITES_N {
            let cy = REST_SITES[i].cyc.load(Ordering::Relaxed);
            if cy == 0 {
                continue;
            }
            for t in 0..top.len() {
                if cy > top_cyc[t] {
                    // shift down and place
                    let mut j = top.len() - 1;
                    while j > t {
                        top[j] = top[j - 1];
                        top_cyc[j] = top_cyc[j - 1];
                        j -= 1;
                    }
                    top[t] = i;
                    top_cyc[t] = cy;
                    break;
                }
            }
        }
        let any_site = top.iter().any(|&t| t != usize::MAX);
        let of_cyc = REST_OF.cyc.load(Ordering::Relaxed);
        if any_site || of_cyc != 0 {
            crate::pr_warn!(
                "SYSCOST t={}s rest-sites: (top by charged cycles; n=opens tot=wall max=longest)",
                secs
            );
        }
        for t in 0..top.len() {
            let i = top[t];
            if i == usize::MAX {
                continue;
            }
            let key = REST_SITES[i].loc.load(Ordering::Relaxed);
            let n = REST_SITES[i].n.load(Ordering::Relaxed).max(1);
            // INV-FOR-3 edge.
            let l = crate::debug::forensics_edges::location(key as usize);
            let (file, line) = (l.file(), l.line());
            crate::pr_warn!(
                "SYSCOST t={}s rest-site {:2}: {}:{} n={} tot={}ms avg={}ns max={}us",
                secs,
                t + 1,
                file,
                line,
                REST_SITES[i].n.load(Ordering::Relaxed),
                top_cyc[t] / msd,
                top_cyc[t] / n * 1000 / (tsc_hz / 1_000_000).max(1),
                REST_SITES[i].max.load(Ordering::Relaxed) / usd
            );
        }
        if of_cyc != 0 || REST_OF.n.load(Ordering::Relaxed) != 0 {
            let n = REST_OF.n.load(Ordering::Relaxed).max(1);
            crate::pr_warn!(
                "SYSCOST t={}s rest-site of: n={} tot={}ms avg={}ns max={}us (table-full + missed-edge opens)",
                secs,
                REST_OF.n.load(Ordering::Relaxed),
                of_cyc / msd,
                of_cyc / n * 1000 / (tsc_hz / 1_000_000).max(1),
                REST_OF.max.load(Ordering::Relaxed) / usd
            );
        }
    }

    // pa15 UART per-byte cost distribution (console-async decision
    // input). Buckets are log2 of raw TSC cycles per putc call.
    {
        let tsc_hz = crate::arch::cpu::get_counter_freq().max(1);
        let msd = tsc_hz / 1000;
        for (tag, s) in [("if0", &UART_IF0), ("if1", &UART_IF1)] {
            let n = s.n.load(Ordering::Relaxed);
            if n == 0 {
                continue;
            }
            crate::pr_warn!(
                "SYSCOST t={}s uart {} : n={} cyc={}ms avg={}ns max={}ms polls={} multi={} ({:.0}%)",
                secs,
                tag,
                n,
                s.cyc.load(Ordering::Relaxed) / msd,
                // avg ns/byte: divide first (cyc * 1e9 overflows u64 once
                // the table passes ~7 s of accumulated TSC).
                s.cyc.load(Ordering::Relaxed) / n.max(1) * 1000
                    / (tsc_hz / 1_000_000).max(1),
                s.max.load(Ordering::Relaxed) / msd,
                s.polls.load(Ordering::Relaxed),
                s.polls_gt1.load(Ordering::Relaxed),
                s.polls_gt1.load(Ordering::Relaxed) as u64 * 100 / n
            );
            let mut line = alloc::string::String::new();
            for b in 0..UART_HIST {
                let c = s.hist[b].load(Ordering::Relaxed);
                if c == 0 {
                    continue;
                }
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(&alloc::format!("2^{}={}", b, c));
            }
            if !line.is_empty() {
                crate::pr_warn!("SYSCOST t={}s uart-hist {}: {}", secs, tag, line);
            }
        }
    }

    // pa15 per-comm printk volume (console producers; "<irq>" collects
    // interrupt-context prints so they do not smear interrupted tasks).
    {
        crate::pr_warn!(
            "SYSCOST t={}s printk: filtered_by_loglevel={}",
            secs,
            PRNT_FILTERED.load(Ordering::Relaxed)
        );
        for i in 0..PRNT.len() {
            let recs = PRNT[i].recs.load(Ordering::Relaxed);
            if recs == 0 {
                continue;
            }
            let mut comm = [0u8; 16];
            for j in 0..16 {
                comm[j] = PRNT[i].comm[j].load(Ordering::Relaxed);
            }
            crate::pr_warn!(
                "SYSCOST t={}s printk comm={} recs={} ringB={} conB={} con_if0B={}",
                secs,
                core::str::from_utf8(&comm).unwrap_or("?").split('\0').next().unwrap_or("?"),
                recs,
                PRNT[i].ring_bytes.load(Ordering::Relaxed),
                PRNT[i].con_bytes.load(Ordering::Relaxed),
                PRNT[i].con_if0_bytes.load(Ordering::Relaxed)
            );
        }
    }

    // pa15 recv-family fd census (hilogd.server dual-drain evidence).
    for i in 0..RECV.len() {
        let n_total = RECV[i].n_total.load(Ordering::Relaxed);
        if n_total == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = RECV[i].comm[j].load(Ordering::Relaxed);
        }
        let mut desc = [0u8; 96];
        let mut dlen = 0;
        for j in 0..96 {
            desc[j] = RECV[i].desc[j].load(Ordering::Relaxed);
            if desc[j] == 0 {
                dlen = j;
                break;
            }
        }
        crate::pr_warn!(
            "SYSCOST t={}s recv comm={} nr={} tot={} fd={} cur={} data={}({}B) eagain={} zero={} err={}({}n) nb={} flags={:#x} ops={:#x} desc={}",
            secs,
            core::str::from_utf8(&comm).unwrap_or("?").split('\0').next().unwrap_or("?"),
            RECV[i].nr.load(Ordering::Relaxed),
            n_total,
            RECV[i].fd.load(Ordering::Relaxed),
            RECV[i].n_cur.load(Ordering::Relaxed),
            RECV[i].n_data.load(Ordering::Relaxed),
            RECV[i].data_bytes.load(Ordering::Relaxed),
            RECV[i].n_eagain.load(Ordering::Relaxed),
            RECV[i].n_zero.load(Ordering::Relaxed),
            RECV[i].err.load(Ordering::Relaxed),
            RECV[i].n_err.load(Ordering::Relaxed),
            RECV[i].nonblock.load(Ordering::Relaxed),
            RECV[i].flags.load(Ordering::Relaxed),
            RECV[i].ops.load(Ordering::Relaxed),
            core::str::from_utf8(&desc[..dlen]).unwrap_or("?")
        );
    }

    // Syscall table: collect (nr, count, cyc, max), sort by cyc desc.
    let mut rows: alloc::vec::Vec<(usize, u64, u64, u64)> = alloc::vec::Vec::new();
    for i in 0..NR_SLOTS {
        let n = TABLE[i].count.load(Ordering::Relaxed);
        if n == 0 {
            continue;
        }
        rows.push((
            i,
            n,
            TABLE[i].cyc.load(Ordering::Relaxed),
            TABLE[i].max.load(Ordering::Relaxed),
        ));
    }
    rows.sort_by(|a, b| b.2.cmp(&a.2));
    let total_cyc: u64 = rows.iter().map(|r| r.2).sum();
    crate::pr_warn!(
        "SYSCOST t={}s syscalls: {} kinds {} calls tot={} ms",
        secs,
        rows.len(),
        rows.iter().map(|r| r.1).sum::<u64>(),
        total_cyc / US / 1000
    );
    for r in rows.iter().take(48) {
        crate::pr_warn!(
            "SYSCOST t={}s nr={} n={} avg={}us max={}ms tot={}ms",
            secs,
            r.0,
            r.1,
            r.2 / US / r.1.max(1), // us
            r.3 / US / 1000,
            r.2 / US / 1000
        );
    }

    for i in 0..EAGAIN_SPIN.len() {
        let n = EAGAIN_SPIN[i].n.load(Ordering::Relaxed);
        if n == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = EAGAIN_SPIN[i].comm[j].load(Ordering::Relaxed);
        }
        let mut path = [0u8; 96];
        let mut plen = 0;
        for j in 0..96 {
            path[j] = EAGAIN_SPIN[i].path[j].load(Ordering::Relaxed);
            if path[j] == 0 {
                plen = j;
                break;
            }
        }
        crate::pr_warn!(
            "SYSCOST t={}s eagain comm={} path={}",
            secs,
            core::str::from_utf8(&comm).unwrap_or("?").split('\0').next().unwrap_or("?"),
            core::str::from_utf8(&path[..plen]).unwrap_or("?")
        );
    }

    // pa4 EOF-spin census table.
    for i in 0..EOF_SPIN.len() {
        let n = EOF_SPIN[i].n.load(Ordering::Relaxed);
        if n == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = EOF_SPIN[i].comm[j].load(Ordering::Relaxed);
        }
        let mut path = [0u8; 96];
        let mut plen = 0;
        for j in 0..96 {
            path[j] = EOF_SPIN[i].path[j].load(Ordering::Relaxed);
            if path[j] == 0 {
                plen = j;
                break;
            }
        }
        crate::pr_warn!(
            "SYSCOST t={}s eofspin comm={} n={} path={}",
            secs,
            core::str::from_utf8(&comm).unwrap_or("?").split('\0').next().unwrap_or("?"),
            n,
            core::str::from_utf8(&path[..plen]).unwrap_or("?")
        );
    }

    // pa9 positive-read census table (which fd returns the data).
    for i in 0..RD_DATA.len() {
        let n_total = RD_DATA[i].n_total.load(Ordering::Relaxed);
        if n_total == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = RD_DATA[i].comm[j].load(Ordering::Relaxed);
        }
        let mut path = [0u8; 96];
        let mut plen = 0;
        for j in 0..96 {
            path[j] = RD_DATA[i].path[j].load(Ordering::Relaxed);
            if path[j] == 0 {
                plen = j;
                break;
            }
        }
        crate::pr_warn!(
            "SYSCOST t={}s rd_data comm={} n_cur={} n_tot={} bytes={} fd={} file_id={} ops={:#x} path={}",
            secs,
            core::str::from_utf8(&comm).unwrap_or("?").split('\0').next().unwrap_or("?"),
            RD_DATA[i].n_cur.load(Ordering::Relaxed),
            n_total,
            RD_DATA[i].bytes.load(Ordering::Relaxed),
            RD_DATA[i].fd.load(Ordering::Relaxed),
            RD_DATA[i].file_id.load(Ordering::Relaxed),
            RD_DATA[i].ops.load(Ordering::Relaxed),
            core::str::from_utf8(&path[..plen]).unwrap_or("?")
        );
    }

    // pa10 failing-read census table (errno + fd resolution state).
    for i in 0..RD_FAIL.len() {
        let n = RD_FAIL[i].n.load(Ordering::Relaxed);
        if n == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = RD_FAIL[i].comm[j].load(Ordering::Relaxed);
        }
        let mut path = [0u8; 96];
        let mut plen = 0;
        for j in 0..96 {
            path[j] = RD_FAIL[i].path[j].load(Ordering::Relaxed);
            if path[j] == 0 {
                plen = j;
                break;
            }
        }
        crate::pr_warn!(
            "SYSCOST t={}s rd_fail comm={} ret={} n={} fd={} fd_state={} ops={:#x} path={}",
            secs,
            core::str::from_utf8(&comm).unwrap_or("?").split('\0').next().unwrap_or("?"),
            RD_FAIL[i].ret.load(Ordering::Relaxed),
            n,
            RD_FAIL[i].fd.load(Ordering::Relaxed),
            RD_FAIL[i].fd_state.load(Ordering::Relaxed),
            RD_FAIL[i].ops.load(Ordering::Relaxed),
            core::str::from_utf8(&path[..plen]).unwrap_or("?")
        );
    }

    // Slow ring (last SLOW_RING outliers, ring order).
    crate::pr_warn!("SYSCOST t={}s slow-ring (>4ms wall):", secs);
    let base = SLOW_IDX.load(Ordering::Relaxed);
    for k in 0..SLOW_RING {
        let i = (base + SLOW_RING - 1 - k) % SLOW_RING; // newest first
        let cyc = SLOW[i].cyc.load(Ordering::Relaxed);
        if cyc == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = SLOW[i].comm[j].load(Ordering::Relaxed);
        }
        let name = core::str::from_utf8(&comm)
            .unwrap_or("?")
            .split('\0')
            .next()
            .unwrap_or("?");
        crate::pr_warn!(
            "SYSCOST t={}s slow nr={} cyc={}us comm={}",
            secs,
            SLOW[i].nr.load(Ordering::Relaxed),
            cyc / US,
            name
        );
    }

    // Kernel-rip page histogram: top 32 by count.
    let mut pages: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();
    for i in 0..RIP_SLOTS {
        let p = RIP_HIST[i].page.load(Ordering::Relaxed);
        if p == 0 {
            continue;
        }
        pages.push((p, RIP_HIST[i].count.load(Ordering::Relaxed)));
    }
    pages.sort_by(|a, b| b.1.cmp(&a.1));
    let tot: u64 = pages.iter().map(|p| p.1).sum();
    crate::pr_warn!(
        "SYSCOST t={}s rip-pages: {} pages {} kern-ticks",
        secs,
        pages.len(),
        tot
    );
    for p in pages.iter().take(32) {
        crate::pr_warn!(
            "SYSCOST t={}s rip page={:#x} n={} ({}%)",
            secs,
            p.0,
            p.1,
            p.1 * 100 / tot.max(1)
        );
    }
}
