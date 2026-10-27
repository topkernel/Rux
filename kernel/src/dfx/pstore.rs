//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! pstore-lite — the last crashes' evidence, kept in reserved RAM and
//! readable through /sys/fs/pstore after the next boot.
//!
//! The reference kernel's pstore/ramoops arrangement: a RAM region the
//! allocator never touches survives the reboot (a guest reset does not
//! clear RAM), the crashing kernel writes its last words into it, and
//! the next boot exposes the records as
//! `/sys/fs/pstore/<type>-<backend>-<id>` (mode 0444 — the reference's
//! pstore_mkfile in fs/pstore/inode.c). Rux implements the console
//! flavor (a panic) and the dmesg flavor (a recoverable oops) in one
//! reserved region, with the reference's multi-dump depth:
//!
//! - **Reserve.** At boot, `reserve(avoid, floor)` carves the 128 KiB
//!   that ends at `avoid`'s page boundary — the DTB's physical base on
//!   this platform. The DTB placement is the anchor that makes the
//!   scheme work: QEMU loads the FDT at a fixed, machine-config-derived
//!   address (the 2 MiB boundary below RAM end on `virt`), so the carve
//!   lands at the SAME physical address every boot of the same machine,
//!   and it sits strictly below the DTB — the next boot's firmware
//!   rewrites the DTB in place, which must never touch the record. (A
//!   naive top-of-RAM carve would land exactly ON the DTB at the
//!   standard `-m` sizes: 1 GiB RAM ends at 0xc0000000 and the FDT sits
//!   at 0xbfe00000.) `memblock_reserve` then keeps the buddy allocator
//!   out (only memblock-free pages are seeded into the zones). The
//!   carve is refused when it would reach below `floor` (the kernel's
//!   static regions) — a memory map too small to carve from gets no
//!   pstore, not a pstore over the kernel. Known cost: a boot with
//!   `-initrd` places the initrd just below the FDT, so QEMU reloads it
//!   over the record area every boot — the record survives only on the
//!   no-initrd standard flows (the rootfs-on-disk images). The x86_64
//!   twin anchors at the kernel image's physical end instead
//!   (`reserve_at` — the plan doc's Tranche-5 design note).
//!
//! - **Ring of N records.** The region is `N_SLOTS` slots of
//!   `SLOT_SIZE` bytes each. Every slot carries the reference
//!   persistent_ram buffer's header shape (`sig`/`start`/`size` —
//!   the reference `struct persistent_ram_buffer`) extended with the
//!   fields a slot ring needs: a persistent sequence number, the
//!   flavor, the timestamp and a checksum (the reference buys torn-write
//!   detection with ECC bytes; Rux buys it with an additive payload sum
//!   and a sig written LAST). A capture writes into the first dead
//!   slot, or the slot with the oldest sequence when all live — the
//!   reference's multi-zone dump reuse rule. The sequence numbers make
//!   the ordering deterministic across reboots: each capture takes
//!   max(live sequences)+1.
//!
//! - **Capture.** The panic handler arms a small staging sink FIRST, so
//!   every byte the panic report prints (header, CSRs, registers, the
//!   symbolized stack) is copied as it goes to the UART. At the end of
//!   the report the record is written into its slot: panic identity,
//!   the staged report, then the newest printk records. The write is
//!   allocation-free end to end (the panic may be the heap's own
//!   failure) and the printk tail is copied under `try_lock` only — a
//!   stopped CPU may hold the ring, and the panic path must still reach
//!   the `panic=` timeout (a marker line replaces the tail in that
//!   case). The signature word is written LAST: a crash inside the
//!   capture leaves no slot the next boot would trust halfway.
//!
//! - **Re-expose.** After the linear map is up, `early_scan()`
//!   validates every slot (sig, version, size sane, checksum) and
//!   remembers the live ones. Unlike the one-record scheme, the scan
//!   does NOT clear anything: a record lives until it is CONSUMED
//!   (read once through /sys/fs/pstore, below) or overwritten by a
//!   later capture — the reference's contract (a record nobody read
//!   survives the next clean boot). sysfs then publishes every live
//!   record newest-first as `console-ramoops-<id>` / `dmesg-ramoops-<id>`
//!   with id 0 = newest of its flavor — the observable convention of
//!   the reference's multi-dump directory (the newest crash's file is
//!   the one a plain `cat` reads).
//!
//! - **Erase on unlink.** A record file lives in /sys/fs/pstore until
//!   userspace unlinks it: `rm` (or unlink(2)) erases the slot and the
//!   file — the reference's ONLY erase path (fs/pstore/inode.c:
//!   pstore_unlink → psi->erase; ramoops.rst: "To delete a stored
//!   record from RAM, simply unlink the respective pstore file").
//!   Reading consumes nothing: a record is re-readable any number of
//!   times (systemd's pstore.service archives then rm's — the flow the
//!   reference's contract serves). A boot that removes nothing keeps
//!   its records for the next boot.

use alloc::vec::Vec;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// The reserved region's size. 128 KiB: four slots of 32 KiB — each
/// slot holds the full staged report (16 KiB) plus a console tail.
pub const PSTORE_RAM_SIZE: usize = 128 * 1024;

/// How many records the ring keeps.
pub const N_SLOTS: usize = 4;

/// One slot's size (header + payload budget).
pub const SLOT_SIZE: usize = PSTORE_RAM_SIZE / N_SLOTS;

/// The staging sink's size (the panic report itself — header, CSRs,
/// registers, stack — is captured here as it prints).
const SINK_SIZE: usize = 16 * 1024;

/// The slot signature: the reference persistent_ram buffer's magic
/// ("DBGC" — `PERSISTENT_RAM_SIG`), written LAST so a torn capture
/// leaves no slot the next boot would trust halfway.
const SIG: u32 = 0x4347_4244;

/// The slot header, 32 bytes, at each slot's start: the reference
/// `struct persistent_ram_buffer`'s `sig`/`start`/`size` shape plus the
/// fields a slot ring needs.
#[repr(C)]
struct SlotHeader {
    /// The reference persistent_ram signature ("DBGC"), written LAST.
    sig: u32,
    /// Additive payload checksum (the reference buys this with ECC).
    crc: u32,
    /// The reference ring's oldest-byte offset — always 0 here: Rux
    /// records are linear, not wrapped.
    start: u32,
    /// Payload length in bytes (after this header).
    size: u32,
    /// Persistent record sequence: max(live)+1 per capture. Orders the
    /// slots across reboots (the reference orders by zone index; Rux
    /// has no fixed crash-to-zone mapping).
    seq: u32,
    /// Record kind: 0 = console (the panic report, exposed as
    /// console-ramoops-N), 1 = dmesg (the oops report, exposed as
    /// dmesg-ramoops-N) — the reference pstore's two ramoops flavors.
    kind: u32,
    /// jiffies at the capture (the record's timestamp).
    jiffies: u32,
    /// Slot format version.
    version: u32,
}

const VERSION: u32 = 3;

/// Console flavor — the panic report (`console-ramoops-N`).
pub const KIND_CONSOLE: u32 = 0;
/// Dmesg flavor — the oops report (`dmesg-ramoops-N`).
pub const KIND_DMESG: u32 = 1;

/// The region's physical base (0 until `reserve()` ran).
static REGION_BASE: AtomicUsize = AtomicUsize::new(0);

/// The live records found by `early_scan()`, one entry per slot.
/// `LIVE_SEQ[i] == 0` means slot i carries no record this boot.
static LIVE_SEQ: [AtomicU32; N_SLOTS] = [const { AtomicU32::new(0) }; N_SLOTS];
/// The live record's kind (valid only while LIVE_SEQ[i] != 0).
static LIVE_KIND: [AtomicU32; N_SLOTS] = [const { AtomicU32::new(0) }; N_SLOTS];

/// The staging sink: the panic report's bytes, ring-buffered (the write
/// index wraps; the newest SINK_SIZE bytes survive).
static SINK: [AtomicU8Cell; SINK_SIZE] = [const { AtomicU8Cell::new() }; SINK_SIZE];
static SINK_AT: AtomicUsize = AtomicUsize::new(0);
/// The sink is armed (between panic-handler entry and the region write).
static SINK_ARMED: AtomicBool = AtomicBool::new(false);

/// A byte cell the panic sink can write without any lock (atomics only;
/// `AtomicU8` directly would also do, this keeps the array decl short).
struct AtomicU8Cell(core::sync::atomic::AtomicU8);

impl AtomicU8Cell {
    const fn new() -> Self {
        Self(core::sync::atomic::AtomicU8::new(0))
    }
}

/// A record `early_scan()` validated, for the sysfs publisher.
#[derive(Clone, Copy, Debug)]
pub struct RecordInfo {
    /// The slot the record lives in (0..N_SLOTS).
    pub slot: usize,
    /// The record's persistent sequence number (higher = newer).
    pub seq: u32,
    /// The record's flavor (KIND_CONSOLE / KIND_DMESG).
    pub kind: u32,
}

/// Compute and remember the region: the `PSTORE_RAM_SIZE` bytes that end
/// at `avoid`'s page boundary (the DTB's physical base — see the module
/// doc for why the carve anchors there instead of the top of RAM).
/// Returns None when the carve would reach below `floor` (the end of
/// the kernel's static regions) or below the start of RAM — a memory map
/// too small to carve from gets no pstore rather than a pstore over the
/// kernel.
pub fn reserve(avoid: usize, floor: usize) -> Option<(usize, usize)> {
    const PAGE: usize = 4096;
    let avoid_page = avoid & !(PAGE - 1);
    let base = avoid_page.saturating_sub(PSTORE_RAM_SIZE);
    if avoid_page <= PSTORE_RAM_SIZE || base <= floor {
        return None;
    }
    REGION_BASE.store(base, Ordering::Release);
    Some((base, PSTORE_RAM_SIZE))
}

/// Remember the region at an explicit base (the x86_64 anchor: the
/// bytes just above the kernel image's physical end — same address
/// every boot of the same build, unlike anything the firmware places;
/// see the plan doc's Tranche-5 design note). Returns None when `base`
/// is not page-aligned — the caller's anchor arithmetic, not a memory
/// map, decides the address here.
pub fn reserve_at(base: usize) -> Option<(usize, usize)> {
    if base % 4096 != 0 {
        return None;
    }
    REGION_BASE.store(base, Ordering::Release);
    Some((base, PSTORE_RAM_SIZE))
}

/// The region's virtual address through the linear map (None before
/// `reserve()` or when the linear map is not up yet).
fn region_virt() -> Option<*mut u8> {
    let base = REGION_BASE.load(Ordering::Acquire);
    if base == 0 {
        return None;
    }
    let va = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(base as u64));
    Some(va.bits() as *mut u8)
}

/// The slot's virtual base.
fn slot_virt(slot: usize) -> Option<*mut u8> {
    region_virt().map(|v| unsafe { v.add(slot * SLOT_SIZE) })
}

/// Read a slot header (plain loads: the previous boot is gone).
///
/// SAFETY: `virt` must be a slot base inside the reserved region.
unsafe fn read_header(virt: *mut u8) -> SlotHeader {
    core::ptr::read_volatile(virt as *const SlotHeader)
}

/// Is this header a record this boot should trust? sig, version, size
/// sane, and the payload checksum must match.
fn header_valid(virt: *mut u8, hdr: &SlotHeader) -> bool {
    if hdr.sig != SIG || hdr.version != VERSION {
        return false;
    }
    if hdr.size as usize > SLOT_SIZE - size_of::<SlotHeader>() {
        return false;
    }
    // SAFETY: the payload is inside the reserved region.
    unsafe {
        let payload =
            core::slice::from_raw_parts(virt.add(size_of::<SlotHeader>()), hdr.size as usize);
        checksum(payload) == hdr.crc
    }
}

/// Additive payload checksum (wrapping u32 sum). Torn-write detection,
/// not cryptography.
fn checksum(bytes: &[u8]) -> u32 {
    let mut sum = 0u32;
    for &b in bytes {
        sum = sum.wrapping_add(b as u32);
    }
    sum
}

/// After the linear map is up: validate every slot and remember the
/// live records. Nothing is cleared — a record lives until it is
/// unlinked through /sys/fs/pstore (the reference's contract) or
/// overwritten by a later capture.
pub fn early_scan() {
    let mut found = 0usize;
    for slot in 0..N_SLOTS {
        let Some(virt) = slot_virt(slot) else {
            return;
        };
        // SAFETY: the slot is reserved RAM mapped by the linear map;
        // the previous boot is the only writer and it is gone.
        let hdr = unsafe { read_header(virt) };
        if !header_valid(virt, &hdr) {
            continue;
        }
        let kind = if hdr.kind == KIND_DMESG {
            KIND_DMESG
        } else {
            KIND_CONSOLE
        };
        LIVE_SEQ[slot].store(hdr.seq, Ordering::Release);
        LIVE_KIND[slot].store(kind, Ordering::Release);
        found += 1;
    }
    // Announce newest-first, one line per record (the common
    // single-record boot keeps the historical announce sentence).
    // Allocation-free on purpose: early_scan can run before the heap
    // is up, and a selection walk over N static slots needs no Vec.
    if found == 0 {
        return;
    }
    let mut announced = [false; N_SLOTS];
    let mut console_id = 0usize;
    let mut dmesg_id = 0usize;
    for _ in 0..found {
        // Pick the unannounced slot with the newest sequence.
        let mut pick = usize::MAX;
        for slot in 0..N_SLOTS {
            if announced[slot] || LIVE_SEQ[slot].load(Ordering::Relaxed) == 0 {
                continue;
            }
            if pick == usize::MAX
                || LIVE_SEQ[slot].load(Ordering::Relaxed) > LIVE_SEQ[pick].load(Ordering::Relaxed)
            {
                pick = slot;
            }
        }
        if pick == usize::MAX {
            break;
        }
        announced[pick] = true;
        let kind = LIVE_KIND[pick].load(Ordering::Relaxed);
        let (bytes, jiffies) = match slot_virt(pick) {
            // SAFETY: the slot was validated above.
            Some(v) => {
                let hdr = unsafe { read_header(v) };
                (hdr.size, hdr.jiffies)
            }
            None => (0, 0),
        };
        let id = if kind == KIND_DMESG {
            let id = dmesg_id;
            dmesg_id += 1;
            id
        } else {
            let id = console_id;
            console_id += 1;
            id
        };
        crate::pr_info!(
            "pstore: previous boot's {} record found ({} bytes) — /sys/fs/pstore/{}, jiffies {}",
            if kind == KIND_DMESG {
                "dmesg"
            } else {
                "console"
            },
            bytes,
            record_name(kind, id),
            jiffies,
        );
    }
}

/// The live records, NEWEST FIRST (descending sequence). Ids for the
/// file names are assigned per flavor from this order: the newest
/// record of a flavor publishes as `<type>-ramoops-0`.
pub fn live_records() -> Vec<RecordInfo> {
    let mut v = Vec::new();
    for slot in 0..N_SLOTS {
        let seq = LIVE_SEQ[slot].load(Ordering::Acquire);
        if seq != 0 {
            v.push(RecordInfo {
                slot,
                seq,
                kind: LIVE_KIND[slot].load(Ordering::Acquire),
            });
        }
    }
    v.sort_by(|a, b| b.seq.cmp(&a.seq));
    v
}

/// The file name a record publishes as — the reference's
/// `<type>-<backend>-<id>`: console-ramoops-N / dmesg-ramoops-N.
pub fn record_name(kind: u32, id: usize) -> alloc::string::String {
    alloc::format!(
        "{}-ramoops-{}",
        if kind == KIND_DMESG {
            "dmesg"
        } else {
            "console"
        },
        id
    )
}

/// The record's bytes for the sysfs file (empty when the slot is dead).
pub fn read_slot(slot: usize) -> Vec<u8> {
    if slot >= N_SLOTS || LIVE_SEQ[slot].load(Ordering::Acquire) == 0 {
        return Vec::new();
    }
    let Some(virt) = slot_virt(slot) else {
        return Vec::new();
    };
    // SAFETY: reserved, mapped, owned by this boot since early_scan
    // validated it; the only writers are a capture (which rewrites the
    // header wholesale) or consume_slot (which kills the sig).
    unsafe {
        let hdr = read_header(virt);
        if hdr.sig != SIG {
            return Vec::new();
        }
        let len = (hdr.size as usize).min(SLOT_SIZE - size_of::<SlotHeader>());
        core::slice::from_raw_parts(virt.add(size_of::<SlotHeader>()), len).to_vec()
    }
}

/// Erase a record (unlink removal): invalidate the slot so the next
/// boot does not re-expose it. Idempotent.
pub fn consume_slot(slot: usize) {
    if slot >= N_SLOTS {
        return;
    }
    LIVE_SEQ[slot].store(0, Ordering::Release);
    if let Some(virt) = slot_virt(slot) {
        // SAFETY: this boot owns the region; only the sig word dies.
        unsafe {
            core::ptr::write_volatile(virt as *mut u32, 0);
        }
        crate::pr_info!("pstore: record erased (slot {})", slot);
    }
}

// ============================================================================
// The panic sink and the capture
// ============================================================================

/// Arm the staging sink (the panic handler's FIRST step, so the whole
/// report is captured as it prints).
pub fn arm_sink() {
    SINK_ARMED.store(true, Ordering::Release);
}

/// One byte of panic output (ConsoleWriter pushes every byte here).
/// Lock-free by construction: plain atomic stores into a static ring.
pub fn sink(byte: u8) {
    if !SINK_ARMED.load(Ordering::Relaxed) {
        return;
    }
    let at = SINK_AT.fetch_add(1, Ordering::Relaxed) % SINK_SIZE;
    SINK[at].0.store(byte, Ordering::Relaxed);
}

/// Write the panic record into a slot. Called at the very end of
/// the panic report (after the stack dump, before the `panic=` policy),
/// so the staged sink holds the whole report.
///
/// Allocation-free by construction: the head is formatted into a stack
/// buffer, the sink is a static array, and the console tail is
/// formatted straight into the slot by `printk::ring_tail_into` —
/// the panic being captured may well be the heap's own failure, so the
/// capture path must not trust the heap with anything.
pub fn panic_capture(info: &core::panic::PanicInfo<'_>) {
    use core::fmt::Write as _;
    let mut hw = HeadWriter { buf: [0; 512], len: 0 };
    let _ = core::write!(hw, "\nPanic: {}\n", info.message());
    if let Some(loc) = info.location() {
        let _ = core::write!(hw, "Location: {}:{}\n", loc.file(), loc.line());
    }
    write_record(KIND_CONSOLE, &hw);
}

/// Write the oops record into a slot (the dmesg flavor). Called
/// from `dfx::oops` after the report printed and the policy decided
/// the oops is recoverable — the record then survives a reboot the
/// task death would otherwise hide. Same allocation-free construction
/// as the panic capture.
pub fn oops_capture(count: u32) {
    use core::fmt::Write as _;
    let mut hw = HeadWriter { buf: [0; 512], len: 0 };
    let _ = core::write!(hw, "\nOops: non-fatal kernel fault [#{}], task killed\n", count);
    write_record(KIND_DMESG, &hw);
}

/// The record writer shared by both captures: head bytes, then the
/// staged sink (the report as printed), then the printk tail — into the
/// chosen slot.
fn write_record(kind: u32, hw: &HeadWriter) {
    // Disarm the sink first: nothing after this point belongs in the
    // record.
    SINK_ARMED.store(false, Ordering::Release);

    let Some(region) = region_virt() else {
        return;
    };

    // Pick the target slot: the first dead one, else the oldest live
    // sequence (the reference's zone-reuse rule). seq = max(live)+1.
    // SAFETY: the region belongs to this dying boot; the header reads
    // are plain volatile loads of memory only this path touches.
    let mut target = 0usize;
    let mut target_dead = false;
    let mut max_seq = 0u32;
    let mut min_seq = u32::MAX;
    for slot in 0..N_SLOTS {
        // SAFETY: see above.
        let hdr = unsafe { read_header(region.add(slot * SLOT_SIZE)) };
        let dead = hdr.sig != SIG || hdr.version != VERSION
            || hdr.size as usize > SLOT_SIZE - size_of::<SlotHeader>();
        if dead {
            if !target_dead {
                target = slot;
                target_dead = true;
            }
            continue;
        }
        max_seq = max_seq.max(hdr.seq);
        if hdr.seq < min_seq {
            min_seq = hdr.seq;
            if !target_dead {
                target = slot;
            }
        }
    }
    let seq = max_seq.wrapping_add(1).max(1);
    // SAFETY: the slot is inside the reserved region owned by this
    // dying boot; nothing else touches it (the other CPUs are already
    // stopped or on their way; a racing capture on another CPU at worst
    // interleaves bytes — the record is a forensic artifact, not a
    // protocol).
    unsafe {
        let virt = region.add(target * SLOT_SIZE);
        let budget = SLOT_SIZE - size_of::<SlotHeader>();
        let limit = virt.addr() + size_of::<SlotHeader>() + budget;
        let mut dst = virt.add(size_of::<SlotHeader>());
        let mut write = |src: &[u8]| {
            let take = src.len().min(limit - dst.addr());
            core::ptr::copy_nonoverlapping(src.as_ptr(), dst, take);
            dst = dst.add(take);
        };

        // 1. The identity head.
        write(&hw.buf[..hw.len]);

        // 2. The staged report — the static sink, copied ring-wise
        // (atomic loads; every byte ever sunk sits at SINK[s %
        // SINK_SIZE], the newest SINK_SIZE bytes are kept, oldest-first).
        write(b"--- report (as printed) ---\n");
        let at = SINK_AT.load(Ordering::Relaxed);
        let first = at.saturating_sub(SINK_SIZE);
        for s in first..at {
            let b = SINK[s % SINK_SIZE].0.load(Ordering::Relaxed);
            if dst.addr() >= limit {
                break;
            }
            core::ptr::write_volatile(dst, b);
            dst = dst.add(1);
        }

        // 3. The console tail, formatted straight into the slot's
        // remaining space (try-lock inside — a stopped CPU may hold the
        // printk ring; the panic path must never wait).
        if dst.addr() < limit {
            let tail = core::slice::from_raw_parts_mut(dst, limit - dst.addr());
            let n = crate::printk::ring_tail_into(tail);
            dst = dst.add(n);
        }
        let length = dst.addr() - virt.addr() - size_of::<SlotHeader>();

        // Signature last-word-first: everything but the sig now, sig
        // LAST — a crash inside this function must not leave a slot
        // the next boot would trust halfway.
        let hdr = virt as *mut SlotHeader;
        let payload = core::slice::from_raw_parts(virt.add(size_of::<SlotHeader>()), length);
        core::ptr::write_volatile(&mut (*hdr).crc, checksum(payload));
        core::ptr::write_volatile(&mut (*hdr).start, 0);
        core::ptr::write_volatile(&mut (*hdr).size, length as u32);
        core::ptr::write_volatile(&mut (*hdr).seq, seq);
        core::ptr::write_volatile(&mut (*hdr).kind, kind);
        core::ptr::write_volatile(
            &mut (*hdr).jiffies,
            crate::drivers::timer::get_jiffies() as u32,
        );
        core::ptr::write_volatile(&mut (*hdr).version, VERSION);
        core::ptr::write_volatile(&mut (*hdr).sig, SIG);
    }
}

/// Stack-buffer fmt sink for the record head (no allocation).
struct HeadWriter {
    buf: [u8; 512],
    len: usize,
}

impl core::fmt::Write for HeadWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The slot header is exactly 32 bytes (8 words) — the region
    /// arithmetic (slot stride, payload budget) depends on it.
    #[test]
    fn test_slot_header_size() {
        assert_eq!(size_of::<SlotHeader>(), 32);
    }

    /// Four slots exactly tile the region.
    #[test]
    fn test_slots_tile_region() {
        assert_eq!(N_SLOTS * SLOT_SIZE, PSTORE_RAM_SIZE);
    }

    /// The checksum is a pure function of the bytes: same bytes, same
    /// sum; different bytes, different sum (for one-byte flips).
    #[test]
    fn test_checksum_sensitivity() {
        assert_eq!(checksum(b"hello"), checksum(b"hello"));
        assert_ne!(checksum(b"hello"), checksum(b"hellp"));
        // Wraps instead of panicking on large inputs.
        let big = [0xffu8; 4096];
        assert_eq!(checksum(&big), checksum(&big));
    }

    /// The reserve carve keeps its floor rule with the new slot sizes
    /// (unchanged region size, so the same arithmetic as before).
    /// REGION_BASE is reset afterwards: it is global state, and the
    /// dead-slot tests below must not see a "reserved" region.
    #[test]
    fn test_reserve_floor() {
        // avoid far above floor: carves.
        assert!(reserve(0x1000_0000, 0x0010_0000).is_some());
        // avoid too close to the floor: refused.
        assert!(reserve(0x1000_0000, 0x0ff0_0000).is_some());
        assert!(reserve(0x0010_0000, 0x0010_0000).is_none());
        // unaligned base: refused.
        assert!(reserve_at(0x1234).is_none());
        REGION_BASE.store(0, Ordering::Release);
    }

    /// Record names follow the reference `<type>-<backend>-<id>`.
    #[test]
    fn test_record_names() {
        assert_eq!(record_name(KIND_CONSOLE, 0), "console-ramoops-0");
        assert_eq!(record_name(KIND_CONSOLE, 2), "console-ramoops-2");
        assert_eq!(record_name(KIND_DMESG, 0), "dmesg-ramoops-0");
    }

    /// read/consume on an unreserved region are quiet no-ops.
    #[test]
    fn test_dead_slot_reads_empty() {
        assert!(read_slot(0).is_empty());
        assert!(read_slot(N_SLOTS).is_empty());
        consume_slot(0); // must not panic
        assert!(live_records().is_empty());
    }
}
