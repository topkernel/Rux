//! dfx=hilogcensus — per-caller census of the OpenHarmony log fabric
//! (hilog-flood lane).
//!
//! The OH boot floods the hilog unix-dgram fabric at 10^4-10^6 msg/s
//! (hilogd.server recvmsg matches the writers' writev ok count 1:1 —
//! every message is one writev(header, tag, content) datagram from
//! libhilog). syscost's 16-slot first-seen comm table cannot name the
//! producers (slots fill early, the big writers print past the printk
//! line truncation), and the in-guest `hilog` CLI returns nothing on
//! this image. This census answers "WHO emits, WHAT tag, at what rate"
//! from the kernel side:
//!
//! - `unix_send_pre/post` — hooks in the writev gathered-dgram
//!   path (syscall/io.rs): parses the 28-byte HilogMsg header
//!   (hilog_base.h: len u16@0; version/type/level/tagLen bitfield u16@2;
//!   tv_sec/nsec, mono_sec, pid, tid, domain; tag at 28) and counts per
//!   SENDER pid (direct-mapped table), keeping a 4-entry tag histogram
//!   per sender, delivered/EAGAIN/other outcome counters, and bytes.
//! - `kmsg_write(buf)` — hook in kmsg_file_write (printk.rs): per-caller
//!   counts of /dev/kmsg writes plus the last line head (init's
//!   LoopEvent-warning channel).
//!
//! Arming (obs-hygiene migration): the unified-framework tracepoint
//! `rux:hilog_census` IS the switch — arm it via `trace_event=` on the
//! cmdline or `set_event` at runtime; one relaxed load per hook when
//! armed, zero when off. Each dumped row also records into the framework
//! (`rux:hilog_census` per unix sender, `rux:hilog_kmsg` per kmsg
//! writer; the old `rux:hilog_top` top-row event keeps firing for the
//! hilog-flood lane's tooling). NOT `hilogtap`, which arms the existing
//! per-record content tap in net/unix.rs — that scanner costs ~seconds
//! of CPU/s at storm rates. The bespoke cmdline token `hilogcensus=1`
//! survives as a compatibility ALIAS that arms the same events at boot
//! (existing hunt rigs keep working; the frozen dfx= switch table is
//! untouched — this module was never in it). Dumped on the syscost
//! periodic cadence (and the shutdown cascade): top senders, one
//! bounded line each. Aggregate-as-event throughout — a per-message
//! tracepoint at 10^6/s would cost more than it observes.
//!
//! Table honesty notes: unix slots are DIRECT-MAPPED by
//! `sender_pid & (SLOTS-1)`; a colliding/reused pid resets the slot
//! (EVICT counts the resets). Rate = n delta between dumps / dump
//! interval, computed host-side from the printed cumulative counts.

// Kernel layer policy (docs/architecture/kernel-layers.md): this module
// is data-plane Safe Rust EXCEPT the two read-only `current task`
// comm/pid snapshots (the syscost flood_account pattern — the same
// single documented unsafe shape, no other unsafe in the module).

use core::sync::atomic::Ordering::Relaxed;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8};

/// The census gate: the unified-framework event switch (live — arming
/// through tracefs `set_event` at runtime takes effect immediately,
/// which the old cached tri-state could not do).
#[inline]
pub fn on() -> bool {
    crate::trace::EVENT_HILOG_CENSUS.armed()
}

/// Compatibility arm (boot, before trace_event= is applied): the old
/// bespoke cmdline token `hilogcensus=1` now just flips the unified
/// events' enable switches — same running state as
/// `trace_event=rux:hilog_census,rux:hilog_kmsg`.
pub fn compat_arm() {
    if crate::cmdline::get_cmdline().is_none() {
        return;
    }
    if crate::cmdline::get_param("hilogcensus").as_deref() == Some("1") {
        crate::trace::EVENT_HILOG_CENSUS.enabled.store(true, Relaxed);
        crate::trace::EVENT_HILOG_KMSG.enabled.store(true, Relaxed);
        crate::pr_warn!(
            "hilogcensus: compat token hilogcensus=1 armed (rux:hilog_census + rux:hilog_kmsg)"
        );
    }
}

const COMM: usize = 16;
const TAG: usize = 16;

struct TagSlot {
    t: [AtomicU8; TAG],
    n: AtomicU64,
    /// Content head (the text after header+tag) of the message that
    /// claimed/last-replaced this slot — the failing-op sample.
    c: [AtomicU8; 48],
    c_len: AtomicU32,
}

impl TagSlot {
    const fn new() -> Self {
        TagSlot {
            t: [const { AtomicU8::new(0) }; TAG],
            n: AtomicU64::new(0),
            c: [const { AtomicU8::new(0) }; 48],
            c_len: AtomicU32::new(0),
        }
    }
    fn bytes(&self) -> [u8; TAG] {
        let mut out = [0u8; TAG];
        for j in 0..TAG {
            out[j] = self.t[j].load(Relaxed);
        }
        out
    }
    fn set(&self, b: &[u8]) {
        for j in 0..TAG {
            let v = *b.get(j).unwrap_or(&0);
            self.t[j].store(v, Relaxed);
        }
    }
    fn same(&self, b: &[u8]) -> bool {
        for j in 0..TAG {
            if self.t[j].load(Relaxed) != *b.get(j).unwrap_or(&0) {
                return false;
            }
        }
        true
    }
    fn set_content(&self, b: &[u8]) {
        let l = b.len().min(48);
        for j in 0..l {
            self.c[j].store(b[j], Relaxed);
        }
        self.c_len.store(l as u32, Relaxed);
    }
    fn content_str(&self) -> alloc::string::String {
        let l = self.c_len.load(Relaxed) as usize;
        let mut out = alloc::string::String::new();
        for j in 0..l.min(48) {
            let b = self.c[j].load(Relaxed);
            if b == b'\n' || b == 0 {
                break;
            }
            if (0x20..0x7f).contains(&b) {
                out.push(b as char);
            } else {
                out.push('.');
            }
        }
        out
    }
}

struct UnixSender {
    pid: AtomicU32,
    /// Header pid != sender pid (appspawn-style relay logging).
    relay: AtomicU64,
    /// Gathered dgrams that did NOT parse as HilogMsg (len<30 or no
    /// header) — other dgram protocols riding the same writev path.
    miss: AtomicU64,
    n: AtomicU64,
    ea: AtomicU64,
    err: AtomicU64,
    bytes: AtomicU64,
    comm: [AtomicU8; COMM],
    tags: [TagSlot; 4],
}

impl UnixSender {
    const fn new() -> Self {
        UnixSender {
            pid: AtomicU32::new(u32::MAX),
            relay: AtomicU64::new(0),
            miss: AtomicU64::new(0),
            n: AtomicU64::new(0),
            ea: AtomicU64::new(0),
            err: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            comm: [const { AtomicU8::new(0) }; COMM],
            tags: [const { TagSlot::new() }; 4],
        }
    }
}

const UNIX_SLOTS: usize = 1024; // direct-mapped by sender pid
static UNIX: [UnixSender; UNIX_SLOTS] = [const { UnixSender::new() }; UNIX_SLOTS];
static EVICT: AtomicU64 = AtomicU64::new(0);

struct KmsgSender {
    pid: AtomicU32,
    n: AtomicU64,
    comm: [AtomicU8; COMM],
    /// Last line head (raw, ≤32 bytes) — the repeating-pattern sample.
    head: [AtomicU8; 32],
    head_len: AtomicU32,
}

impl KmsgSender {
    const fn new() -> Self {
        KmsgSender {
            pid: AtomicU32::new(u32::MAX),
            n: AtomicU64::new(0),
            comm: [const { AtomicU8::new(0) }; COMM],
            head: [const { AtomicU8::new(0) }; 32],
            head_len: AtomicU32::new(0),
        }
    }
}

const KMSG_SLOTS: usize = 256;
static KMSG: [KmsgSender; KMSG_SLOTS] = [const { KmsgSender::new() }; KMSG_SLOTS];

/// Parse the packed HilogMsg header out of a gathered dgram.
/// Returns (header_pid, level, domain, tag bytes).
fn parse_hilogmsg(b: &[u8]) -> Option<(u32, u8, u32, [u8; TAG], &[u8])> {
    if b.len() < 30 {
        return None;
    }
    let rd16 = |o: usize| -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) };
    let rd32 = |o: usize| -> u32 {
        u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    };
    let bf = rd16(2);
    let level = (bf >> 7 & 0x7) as u8;
    let tag_len = (bf >> 10 & 0x3f) as usize;
    let pid = rd32(16);
    let domain = rd32(24);
    let content_at = 28 + tag_len;
    let content = b.get(content_at..).unwrap_or(&[]);
    if tag_len == 0 || tag_len > 63 {
        return Some((pid, level, domain, [0u8; TAG], content));
    }
    let mut tag = [0u8; TAG];
    let tl = tag_len.min(TAG - 1).max(1); // drop the trailing NUL room
    for j in 0..tl {
        tag[j] = *b.get(28 + j).unwrap_or(&0);
    }
    Some((pid, level, domain, tag, content))
}

/// Per-CPU slot stash: unix_send_pre hands the claimed slot index to the
/// matching unix_send_post. The two calls bracket the owned send (the
/// gathered Vec is MOVED into unix_write_slice_owned, so the census
/// parses BEFORE and records the outcome AFTER). Same task issues both;
/// a migration between the two adjacent statements could mis-attribute
/// one outcome — bounded noise at census granularity, noted here.
static PRE_SLOT: [AtomicU32; crate::config::MAX_CPUS] =
    [const { AtomicU32::new(u32::MAX) }; crate::config::MAX_CPUS];

/// Hook half 1 (BEFORE the owned send): parse the gathered datagram,
/// claim/refresh the sender slot, record bytes + tag.
pub fn unix_send_pre(gathered: &[u8]) {
    if !on() {
        return;
    }
    let (spid, scomm): (u32, [u8; COMM]) = match crate::sched::current() {
        // SAFETY: read-only comm/pid access on the current task.
        Some(t) => unsafe { ((*t).pid() as u32, *(*t).comm()) },
        None => return,
    };
    let hdr = parse_hilogmsg(gathered);
    let si = (spid as usize) & (UNIX_SLOTS - 1);
    let slot = &UNIX[si];
    if slot.pid.load(Relaxed) != spid {
        // Fresh claim (or pid collision/reuse): reset the slot. The lost
        // counts are the direct-map trade-off; EVICT tallies it.
        EVICT.fetch_add(1, Relaxed);
        slot.pid.store(spid, Relaxed);
        slot.n.store(0, Relaxed);
        slot.ea.store(0, Relaxed);
        slot.err.store(0, Relaxed);
        slot.bytes.store(0, Relaxed);
        slot.relay.store(0, Relaxed);
        slot.miss.store(0, Relaxed);
        for j in 0..COMM {
            slot.comm[j].store(scomm[j], Relaxed);
        }
        for tg in &slot.tags {
            tg.n.store(0, Relaxed);
            for j in 0..TAG {
                tg.t[j].store(0, Relaxed);
            }
        }
    }
    slot.bytes.fetch_add(gathered.len() as u64, Relaxed);
    match hdr {
        Some((hpid, _level, _domain, tag, content)) => {
            if hpid != 0 && hpid != spid {
                slot.relay.fetch_add(1, Relaxed);
            }
            bump_tag(&slot.tags, &tag, content);
        }
        None => {
            slot.miss.fetch_add(1, Relaxed);
        }
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::config::MAX_CPUS {
        PRE_SLOT[cpu].store(si as u32, Relaxed);
    }
}

/// 4-entry tag histogram: match | claim-empty | replace-min. The content
/// head rides the claim/replace (the failing-op sample per tag).
fn bump_tag(tags: &[TagSlot; 4], tag: &[u8; TAG], content: &[u8]) {
    let mut min_i = 0usize;
    let mut min_n = u64::MAX;
    for (i, t) in tags.iter().enumerate() {
        let n = t.n.load(Relaxed);
        if t.same(tag) {
            t.n.fetch_add(1, Relaxed);
            return;
        }
        if n == 0 {
            t.set(tag);
            t.set_content(content);
            t.n.store(1, Relaxed);
            return;
        }
        if n < min_n {
            min_n = n;
            min_i = i;
        }
    }
    tags[min_i].set(tag);
    tags[min_i].set_content(content);
    tags[min_i].n.store(1, Relaxed);
}

/// Hook half 2 (AFTER the owned send): record the outcome by kernel
/// return value (>=0 delivered, -11 EAGAIN, else error).
pub fn unix_send_post(ret: isize) {
    if !on() {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    let si = PRE_SLOT[cpu].swap(u32::MAX, Relaxed);
    if si == u32::MAX {
        return;
    }
    let slot = &UNIX[si as usize];
    if ret >= 0 {
        slot.n.fetch_add(1, Relaxed);
    } else if ret == -11 {
        slot.ea.fetch_add(1, Relaxed);
    } else {
        slot.err.fetch_add(1, Relaxed);
    }
}

/// Hook: one userspace /dev/kmsg write (printk.rs kmsg_file_write).
pub fn kmsg_write(buf: &[u8]) {
    if !on() {
        return;
    }
    let (spid, scomm): (u32, [u8; COMM]) = match crate::sched::current() {
        // SAFETY: read-only comm/pid access on the current task.
        Some(t) => unsafe { ((*t).pid() as u32, *(*t).comm()) },
        None => return,
    };
    let slot = &KMSG[(spid as usize) & (KMSG_SLOTS - 1)];
    if slot.pid.load(Relaxed) != spid {
        slot.pid.store(spid, Relaxed);
        slot.n.store(0, Relaxed);
        for j in 0..COMM {
            slot.comm[j].store(scomm[j], Relaxed);
        }
    }
    slot.n.fetch_add(1, Relaxed);
    let hl = buf.len().min(32);
    for j in 0..hl {
        slot.head[j].store(buf[j], Relaxed);
    }
    slot.head_len.store(hl as u32, Relaxed);
}

fn comm_str(c: &[AtomicU8; COMM]) -> alloc::string::String {
    let mut out = alloc::string::String::new();
    for j in 0..COMM {
        let b = c[j].load(Relaxed);
        if b == 0 {
            break;
        }
        out.push(b as char);
    }
    if out.is_empty() {
        out.push('?');
    }
    out
}

fn tag_str(t: &TagSlot) -> alloc::string::String {
    let b = t.bytes();
    let end = b.iter().position(|&x| x == 0).unwrap_or(TAG);
    if end == 0 {
        return "?".into();
    }
    core::str::from_utf8(&b[..end]).unwrap_or("?").into()
}

/// Periodic dump (syscost cadence + shutdown cascade).
pub fn dump() {
    if !on() {
        return;
    }
    let secs = crate::drivers::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;
    // Top-8 unix senders by n. Bounded scratch, no allocation tricks.
    let mut idx: [usize; 8] = [usize::MAX; 8];
    let mut top: [u64; 8] = [0; 8];
    for i in 0..UNIX_SLOTS {
        let n = UNIX[i].n.load(Relaxed) + UNIX[i].ea.load(Relaxed);
        if n == 0 {
            continue;
        }
        // insertion into the running top-8
        for k in 0..8 {
            if n > top[k] {
                // shift down
                let mut m = 7;
                while m > k {
                    idx[m] = idx[m - 1];
                    top[m] = top[m - 1];
                    m -= 1;
                }
                idx[k] = i;
                top[k] = n;
                break;
            }
        }
    }
    crate::pr_warn!(
        "HILOGTAP t={}s unix top senders (ok+eagain cumulative; evicts={}):",
        secs,
        EVICT.load(Relaxed)
    );
    for k in 0..8 {
        let i = match idx[k] {
            usize::MAX => continue,
            i => i,
        };
        let s = &UNIX[i];
        let n = s.n.load(Relaxed);
        let ea = s.ea.load(Relaxed);
        if n + ea == 0 {
            continue;
        }
        let bytes = s.bytes.load(Relaxed);
        let avg = if n > 0 { bytes / n } else { 0 };
        // One bounded line: pid comm n ea err avgB relay tags(2 shown)
        let t0 = tag_str(&s.tags[0]);
        let t1 = tag_str(&s.tags[1]);
        let l0 = s.tags[0].n.load(Relaxed);
        let l1 = s.tags[1].n.load(Relaxed);
        crate::pr_warn!(
            "HILOGTAP pid={} {} n={} ea={} err={} avgB={} relay={} miss={} tags='{}':{},'{}':{}",
            s.pid.load(Relaxed),
            comm_str(&s.comm),
            n,
            ea,
            s.err.load(Relaxed),
            avg,
            s.relay.load(Relaxed),
            s.miss.load(Relaxed),
            t0,
            l0,
            t1,
            l1,
        );
        crate::pr_warn!(
            "HILOGTAP   c0='{}'",
            s.tags[0].content_str(),
        );
        // obs-hygiene migration: the same row records into the unified
        // framework (rux:hilog_census — the event is also the census's
        // arm switch, so it is armed by construction here).
        {
            let mut c16 = [0u8; crate::trace::COMM_LEN];
            for j in 0..crate::trace::COMM_LEN.min(COMM) {
                c16[j] = s.comm[j].load(Relaxed);
            }
            crate::trace::trace_hilog_census(
                s.pid.load(Relaxed) as i32,
                &c16,
                n,
                ea,
                s.err.load(Relaxed),
                bytes,
                s.relay.load(Relaxed),
                s.miss.load(Relaxed),
            );
        }
        // Aggregate-as-event: the top row rides the unified framework.
        if k == 0 && crate::trace::EVENT_HILOG_TOP.armed() {
            crate::trace::trace_hilog_top(
                s.pid.load(Relaxed) as i32,
                &{
                    let mut c = [0u8; crate::trace::COMM_LEN];
                    for j in 0..crate::trace::COMM_LEN.min(COMM) {
                        c[j] = s.comm[j].load(Relaxed);
                    }
                    c
                },
                n,
                ea,
            );
        }
    }
    crate::pr_warn!("HILOGTAP kmsg writers:");
    let mut shown = 0;
    for i in 0..KMSG_SLOTS {
        let s = &KMSG[i];
        let n = s.n.load(Relaxed);
        if n == 0 || s.pid.load(Relaxed) == u32::MAX {
            continue;
        }
        let hl = s.head_len.load(Relaxed) as usize;
        let mut head = [0u8; 32];
        let mut hn = 0usize;
        for j in 0..hl.min(32) {
            let b = s.head[j].load(Relaxed);
            if b == b'\n' || b == 0 {
                break;
            }
            if (0x20..0x7f).contains(&b) {
                head[hn] = b;
            } else {
                head[hn] = b'.';
            }
            hn += 1;
        }
        crate::pr_warn!(
            "HILOGTAP kmsg pid={} {} n={} head='{}'",
            s.pid.load(Relaxed),
            comm_str(&s.comm),
            n,
            alloc::string::String::from_utf8_lossy(&head[..hn])
        );
        // obs-hygiene migration: the kmsg writer row records into the
        // unified framework (rux:hilog_kmsg carries the 32-byte head
        // sample — the repeating-pattern evidence).
        if crate::trace::EVENT_HILOG_KMSG.armed() {
            let mut c16 = [0u8; crate::trace::COMM_LEN];
            for j in 0..crate::trace::COMM_LEN.min(COMM) {
                c16[j] = s.comm[j].load(Relaxed);
            }
            crate::trace::trace_hilog_kmsg(
                s.pid.load(Relaxed) as i32,
                &c16,
                n,
                &head,
            );
        }
        shown += 1;
        if shown >= 6 {
            break;
        }
    }
    if shown == 0 {
        crate::pr_warn!("HILOGTAP kmsg (none)");
    }
}
