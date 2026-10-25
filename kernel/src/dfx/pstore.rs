//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! pstore-lite — the last panic's evidence, kept in reserved RAM and
//! readable through /sys/fs/pstore after the next boot.
//!
//! The reference kernel's pstore/ramoops arrangement: a RAM region the
//! allocator never touches survives the reboot (a guest reset does not
//! clear RAM), the crashing kernel writes its last words into it, and
//! the next boot exposes the record as
//! `/sys/fs/pstore/<type>-<backend>-<id>` (mode 0444 — the reference's
//! pstore_mkfile in fs/pstore/inode.c). Rux implements the console
//! flavor of that with one reserved region:
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
//!   no-initrd standard flows (the rootfs-on-disk images).
//!
//! - **Capture.** The panic handler arms a small staging sink FIRST, so
//!   every byte the panic report prints (header, CSRs, registers, the
//!   symbolized stack) is copied as it goes to the UART. At the end of
//!   the report `panic_capture()` writes the region: a fixed header
//!   (magic, version, length, jiffies), the staged panic report, then
//!   the newest printk records (a panic-context, lock-free tail walk —
//!   a stopped CPU may hold the ring lock, so the copy must not wait).
//!   The magic word is written LAST: a crash inside the capture leaves
//!   no half-record for the next boot to trust.
//!
//! - **Re-expose.** After the linear map is up, `early_scan()` checks
//!   the magic. A valid record is remembered and the magic is cleared
//!   immediately (the CONTENT stays — only the marker goes; without the
//!   clear, a clean next boot would re-expose the same stale record).
//!   sysfs then publishes it as `console-ramoops-0` when a record was
//!   found — the reference names files `<type>-<backend>-<id>` and the
//!   single console zone carries id 0.
//!
//! The region is one record deep: the next panic overwrites it, the
//! same last-panic contract the reference's console backend has.

use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The reserved region's size. 128 KiB: the panic report (≤ the staging
/// sink) plus ~110 KiB of console tail — several full screens of
/// context around the crash.
pub const PSTORE_RAM_SIZE: usize = 128 * 1024;

/// The staging sink's size (the panic report itself — header, CSRs,
/// registers, stack — is captured here as it prints).
const SINK_SIZE: usize = 16 * 1024;

/// The record magic: "RUXPST01".
const MAGIC: u64 = u64::from_le_bytes(*b"RUXPST01");

/// The record header, 32 bytes, at the region start.
#[repr(C)]
struct RecordHeader {
    magic: u64,
    version: u32,
    /// Payload length in bytes (after this header).
    length: u32,
    /// jiffies at the panic (the record's timestamp).
    jiffies: u32,
    _reserved: [u32; 3],
}

const VERSION: u32 = 1;

/// The region's physical base (0 until `reserve()` ran).
static REGION_BASE: AtomicUsize = AtomicUsize::new(0);

/// A record from the previous boot was found by `early_scan()`.
static HAS_RECORD: AtomicBool = AtomicBool::new(false);

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

/// Compute and remember the region: the `PSTORE_RAM_SIZE` bytes that end
/// at `avoid`'s page boundary (the DTB's physical base — see the module
/// doc for why the carve anchors there instead of the top of RAM).
/// Returns None when the carve would reach below `floor` (the end of the
/// kernel's static regions) or below the start of RAM — a memory map too
/// small to carve from gets no pstore rather than a pstore over the
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

/// After the linear map is up: look for the previous boot's record.
///
/// A valid magic means the last boot panicked (the panic path is the
/// only writer). Remember it, clear the magic — the content stays
/// readable until the next panic overwrites the region.
pub fn early_scan() {
    let Some(virt) = region_virt() else {
        return;
    };
    // SAFETY: the region is reserved RAM, mapped by the linear map; the
    // header reads are plain loads of memory no writer touches (the
    // previous boot is gone).
    let hdr = unsafe { &*(virt as *const RecordHeader) };
    if hdr.magic != MAGIC || hdr.version != VERSION {
        return;
    }
    let length = (hdr.length as usize).min(PSTORE_RAM_SIZE - size_of::<RecordHeader>());
    HAS_RECORD.store(true, Ordering::Release);
    // SAFETY: same region; this boot now owns it. Clear only the magic.
    unsafe {
        core::ptr::write_volatile(virt as *mut u64, 0);
    }
    crate::pr_info!(
        "pstore: previous boot's panic record found ({} bytes) — /sys/fs/pstore/console-ramoops-0, jiffies {}",
        length,
        hdr.jiffies
    );
}

/// Did this boot find a record? (sysfs asks before publishing the file.)
pub fn has_record() -> bool {
    HAS_RECORD.load(Ordering::Acquire)
}

/// The record's bytes for the sysfs file (empty when there is none).
pub fn read_record() -> alloc::vec::Vec<u8> {
    if !has_record() {
        return alloc::vec::Vec::new();
    }
    let Some(virt) = region_virt() else {
        return alloc::vec::Vec::new();
    };
    // SAFETY: reserved, mapped, owned by this boot since early_scan
    // cleared the magic; no writer exists until the next panic (and a
    // next panic would overwrite with a full new header first).
    let hdr = unsafe { &*(virt as *const RecordHeader) };
    let length = (hdr.length as usize).min(PSTORE_RAM_SIZE - size_of::<RecordHeader>());
    let bytes = unsafe {
        core::slice::from_raw_parts(virt.add(size_of::<RecordHeader>()), length)
    };
    bytes.to_vec()
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

/// Write the record into the region. Called at the very end of the
/// panic report (after the stack dump, before the `panic=` policy), so
/// the staged sink holds the whole report.
///
/// Allocation-free by construction: the head is formatted into a stack
/// buffer, the sink is a static array, and the console tail is
/// formatted straight into the region by `printk::ring_tail_into` —
/// the panic being captured may well be the heap's own failure, so the
/// capture path must not trust the heap with anything.
pub fn panic_capture(info: &core::panic::PanicInfo<'_>) {
    let Some(virt) = region_virt() else {
        return;
    };
    // Disarm the sink first: nothing after this point belongs in the
    // record.
    SINK_ARMED.store(false, Ordering::Release);

    // SAFETY: reserved, mapped RAM owned by this dying boot; nothing
    // else touches it (the other CPUs are already stopped or on their
    // way; a racing panic on another CPU at worst interleaves bytes —
    // the record is a forensic artifact, not a protocol).
    unsafe {
        let budget = PSTORE_RAM_SIZE - size_of::<RecordHeader>();
        let limit = virt.addr() + size_of::<RecordHeader>() + budget;
        let mut dst = virt.add(size_of::<RecordHeader>());
        let mut write = |src: &[u8]| {
            let take = src.len().min(limit - dst.addr());
            core::ptr::copy_nonoverlapping(src.as_ptr(), dst, take);
            dst = dst.add(take);
        };

        // 1. The identity: message + location, via a stack buffer.
        use core::fmt::Write as _;
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
        let mut hw = HeadWriter { buf: [0; 512], len: 0 };
        let _ = core::write!(hw, "\nPanic: {}\n", info.message());
        if let Some(loc) = info.location() {
            let _ = core::write!(hw, "Location: {}:{}\n", loc.file(), loc.line());
        }
        write(&hw.buf[..hw.len]);

        // 2. The staged panic report — the static sink, copied ring-wise
        // (atomic loads; byte s ever sunk sits at SINK[s % SINK_SIZE],
        // the newest SINK_SIZE bytes are kept, oldest-first).
        write(b"--- panic report (as printed) ---\n");
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

        // 3. The console tail, formatted straight into the region's
        // remaining space (try-lock inside — a stopped CPU may hold the
        // printk ring; the panic path must never wait).
        if dst.addr() < limit {
            let tail = core::slice::from_raw_parts_mut(dst, limit - dst.addr());
            let n = crate::printk::ring_tail_into(tail);
            dst = dst.add(n);
        }
        let length = dst.addr() - virt.addr() - size_of::<RecordHeader>();

        // Header last-word-first: everything but the magic now, magic
        // LAST — a crash inside this function must not leave a record
        // the next boot would trust halfway.
        let hdr = virt as *mut RecordHeader;
        core::ptr::write_volatile(&mut (*hdr).version, VERSION);
        core::ptr::write_volatile(
            &mut (*hdr).jiffies,
            crate::drivers::timer::get_jiffies() as u32,
        );
        core::ptr::write_volatile(&mut (*hdr).length, length as u32);
        core::ptr::write_volatile(&mut (*hdr)._reserved, [0; 3]);
        core::ptr::write_volatile(&mut (*hdr).magic, MAGIC);
    }
}

