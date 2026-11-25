//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! uaccess bounds-check property tests (kernel Framework rail #1).
//!
//! `access_ok` is the pure-arithmetic gate every user pointer must pass
//! before the Framework's raw copy routines run; `clamp_rw_count` is the
//! syscall-side transfer-size clamp. Together they bound every uaccess
//! transfer from above (kernel space) and below (null page). These tests
//! pin the arithmetic so a refactor of either function cannot silently
//! admit kernel-space or wrapping ranges.
//!
//! Logic copied from:
//! - kernel/core/src/arch/riscv64/uaccess.rs (access_ok, MAX_RW_COUNT)
//! - kernel/core/src/arch/riscv64/mm/memory_layout.rs (USER_START/USER_END,
//!   TASK_SIZE, KERNEL_LINK_ADDR)
//! - kernel/core/src/syscall/io.rs (clamp_rw_count)
//! When the kernel versions change, these copies must be updated.

use proptest::prelude::*;

// ============================================================================
// Copied constants (kernel/core/src/arch/riscv64/mm/memory_layout.rs)
// ============================================================================

/// Sv39: PGDIR_SIZE = 1 GiB, PTRS_PER_PGD = 512, user half = 256 GiB.
pub const TASK_SIZE: usize = ((1u64 << 30) * 512 / 2) as usize;
pub const USER_START: usize = 0x0000_0000;
pub const USER_END: usize = TASK_SIZE;
pub const KERNEL_LINK_ADDR: usize = 0xffff_ffff_8000_0000;

// The kernel isolation boundary, checked at compile time of this test:
// the entire user range accepted by access_ok lies strictly below the
// kernel's link address.
const _: () = assert!(USER_END < KERNEL_LINK_ADDR);

// ============================================================================
// Copied logic (kernel/core/src/arch/riscv64/uaccess.rs)
// ============================================================================

pub const MAX_RW_COUNT: usize = 0x7FFF_F000;

pub fn access_ok(addr: usize, size: usize) -> bool {
    if addr < USER_START {
        return false;
    }
    let end = match addr.checked_add(size) {
        Some(e) => e,
        None => return false,
    };
    end <= USER_END
}

// ============================================================================
// Copied logic (kernel/core/src/syscall/io.rs)
// ============================================================================

pub fn clamp_rw_count(count: usize) -> usize {
    if count > MAX_RW_COUNT { MAX_RW_COUNT } else { count }
}

// ============================================================================
// Strategies
// ============================================================================

/// usize mix biased to the boundaries that matter: 0, USER_END +/- small,
/// TASK_SIZE, MAX_RW_COUNT neighborhood, and usize::MAX wrap territory.
fn addr_strategy() -> impl Strategy<Value = usize> {
    prop_oneof![
        4 => any::<usize>(),
        1 => (USER_END as u32..=(USER_END as u32).saturating_add(16))
            .prop_map(|v| v as usize),
        1 => 0usize..64,
        1 => (USER_END - 64..=USER_END),
        1 => (usize::MAX - 64..=usize::MAX),
    ]
}

fn size_strategy() -> impl Strategy<Value = usize> {
    prop_oneof![
        4 => any::<usize>(),
        1 => 0usize..64,
        1 => (MAX_RW_COUNT - 16..=MAX_RW_COUNT + 16),
        1 => (usize::MAX - 64..=usize::MAX),
    ]
}

// ============================================================================
// Properties
// ============================================================================

/// Acceptance implies containment with no wrap: [addr, addr+size) lies in
/// [USER_START, USER_END]. This is the single invariant the Framework's raw
/// copy routines rely on.
proptest! {
    #[test]
    fn accepted_ranges_lie_in_user_space(addr in addr_strategy(), size in size_strategy()) {
        if access_ok(addr, size) {
            prop_assert!(addr >= USER_START);
            let end = addr.checked_add(size).expect("accepted range must not wrap");
            prop_assert!(end <= USER_END);
        }
    }
}

/// Anything at or above the kernel half boundary is rejected for every size
/// (the user-writable window can never reach kernel space).
proptest! {
    #[test]
    fn kernel_space_is_never_accepted(
        addr in (USER_END..=usize::MAX),
        size in size_strategy(),
    ) {
        prop_assert!(!access_ok(addr, size));
    }
}

/// Monotone in size: shrinking an accepted transfer keeps it accepted.
proptest! {
    #[test]
    fn acceptance_is_monotone_in_size(
        addr in addr_strategy(),
        size in size_strategy(),
        frac in 0usize..=100,
    ) {
        let shrink = size / 100 * frac;
        if access_ok(addr, size) {
            prop_assert!(access_ok(addr, size - shrink));
        }
    }
}

/// Interval join: a transfer is accepted iff it can be split into two
/// accepted halves (both directions), for non-wrapping base addresses.
proptest! {
    #[test]
    fn interval_split_join(
        addr in addr_strategy(),
        s1 in size_strategy(),
        s2 in size_strategy(),
    ) {
        prop_assume!(addr <= USER_END);
        let joined_ok = s1.checked_add(s2).map(|sum| access_ok(addr, sum)).unwrap_or(false);
        let split_ok = access_ok(addr, s1)
            && addr.checked_add(s1).map(|mid| access_ok(mid, s2)).unwrap_or(false);
        prop_assert_eq!(joined_ok, split_ok);
    }
}

/// Boundary pins.
#[test]
fn access_ok_boundaries() {
    assert!(access_ok(USER_START, 0));
    assert!(access_ok(USER_END, 0));
    assert!(!access_ok(USER_END, 1));
    assert!(!access_ok(USER_END + 1, 0));
    assert!(access_ok(USER_START, USER_END)); // full user window
    assert!(!access_ok(usize::MAX, 1)); // must not wrap
    assert!(!access_ok(usize::MAX, 0)); // above USER_END
    assert!(!access_ok(0, usize::MAX)); // end wraps past USER_END
}

/// Clamp invariants: bounded, idempotent, identity below the bound.
proptest! {
    #[test]
    fn clamp_is_bounded_idempotent_identity(count in any::<usize>()) {
        let c1 = clamp_rw_count(count);
        prop_assert!(c1 <= MAX_RW_COUNT);
        prop_assert_eq!(clamp_rw_count(c1), c1);
        if count <= MAX_RW_COUNT {
            prop_assert_eq!(c1, count);
        } else {
            prop_assert_eq!(c1, MAX_RW_COUNT);
        }
    }
}

/// Pair property: the clamped count can never push an in-range base address
/// out of the user window by itself (clamp bound 2 GiB is far below the
/// 256 GiB user window).
proptest! {
    #[test]
    fn clamped_count_never_causes_rejection_by_size(
        addr in 0usize..USER_END,
        count in any::<usize>(),
    ) {
        let clamped = clamp_rw_count(count);
        if addr + clamped > USER_END {
            // rejection, when it happens, is due to addr being near the top;
            // clamped itself is <= 2 GiB so addr must be within 2 GiB of the
            // top of user space
            prop_assert!(addr + MAX_RW_COUNT > USER_END);
        }
    }
}

// ============================================================================
// Copied logic (kernel/core/src/arch/riscv64/uaccess.rs) — the typed and
// string accessors. Bodies are text-faithful; the extable-backed leaf
// primitives (__copy_from_user / copy_from_user / copy_to_user) are
// replaced by the model below so the loop/gate logic runs on the host.
// ============================================================================

use std::cell::RefCell;

/// Sparse user memory + a fault window: the host stand-in for the
/// exception-table copy primitives. Addresses are absolute integers (the
/// mirror fns take raw pointers whose integer value keys the model).
/// User-memory model.
///
/// The kernel text validates the user pointer's INTEGER value against
/// the kernel's user window (access_ok(from as usize, ...)) and drives
/// its limit arithmetic from it — a host heap pointer (≈0x5555…) can
/// never satisfy that. The model therefore separates the two roles:
///
/// * VALUE: the MODEL_BASE-space address the mirrored text reasons
///   about. The mirrors read it through [`model_addr_of`], a platform
///   hook the sync-checker canonicalizes back to `from as usize`
///   (v_transforms) so the compared text stays kernel-faithful.
/// * PROVENANCE: pointers handed to the mirrors derive from a real
///   allocation (the leaked backing slice), so the mirrored text's
///   `from.add(done)` arithmetic stays Miri-well-defined. The backing
///   is far larger than the model window so the additive offsets in the
///   string loops stay in-bounds.
pub struct UserMemModel {
    /// First faulting byte offset within the window; None = no fault.
    pub fault_off: Option<usize>,
}

thread_local! {
    /// Real backing allocation (64 KiB ≫ the 8 KiB model window), stable
    /// for the life of the test thread once initialized.
    static BACKING: RefCell<Box<[u8]>> = RefCell::new(vec![0u8; BACKING_SPAN].into_boxed_slice());
    static MODEL: RefCell<UserMemModel> = RefCell::new(UserMemModel { fault_off: None });
}

/// Install a fresh model state (fault window only; memory content is
/// written through [`backing_set`]).
pub fn model_setup(fault_at: Option<usize>) {
    let fault_off = fault_at.map(|a| a - MODEL_BASE);
    MODEL.with(|m| {
        *m.borrow_mut() = UserMemModel { fault_off };
    });
}

/// Write bytes at a MODEL_BASE-space address (tests set up user memory
/// through this; reads/writes by the mirrors land on the same bytes).
pub fn backing_set(addr: usize, data: &[u8]) {
    let off = addr - MODEL_BASE;
    BACKING.with(|b| {
        let mut b = b.borrow_mut();
        b[off..off + data.len()].copy_from_slice(data);
    });
}

/// Read bytes at a MODEL_BASE-space address (assertion helper).
pub fn backing_get(addr: usize, n: usize) -> Vec<u8> {
    let off = addr - MODEL_BASE;
    BACKING.with(|b| b.borrow()[off..off + n].to_vec())
}

/// Platform hook: the MODEL_BASE-space value of a backing-window
/// pointer. The sync-checker's v_transforms rewrite this call back to
/// `X as usize` when comparing against the kernel text.
fn model_addr_of<T>(p: *const T) -> usize {
    BACKING.with(|b| {
        let base = b.borrow().as_ptr() as usize;
        let val = p as usize;
        if val >= base && val - base < BACKING_SPAN {
            // Backing-window pointer: the model-space value the mirrored
            // text reasons about.
            MODEL_BASE + (val - base)
        } else {
            // Not a backing pointer (boundary tests pass e.g. usize::MAX
            // to prove the gate rejects it): pass the integer value
            // through unchanged — access_ok must reject it without any
            // model translation, exactly like the kernel-side
            // `p as usize` does for a hostile user value.
            val
        }
    })
}

/// Real pointer for a MODEL_BASE-space address (provenance-carrying:
/// derived from the backing allocation; stays in-bounds for any offset
/// the mirrors add, because the allocation is far larger than the
/// window).
fn real_ptr(addr: usize) -> *const u8 {
    BACKING.with(|b| {
        let base = b.borrow().as_ptr() as usize;
        assert!(addr >= MODEL_BASE && addr < MODEL_BASE + MODEL_SPAN, "addr outside the model window");
        unsafe { (base as *const u8).add(addr - MODEL_BASE) }
    })
}

/// Mutable variant of real_ptr for put_user targets.
fn real_ptr_mut(addr: usize) -> *mut u8 {
    BACKING.with(|b| {
        let mut b = b.borrow_mut();
        let base = b.as_ptr() as usize;
        assert!(addr >= MODEL_BASE && addr < MODEL_BASE + MODEL_SPAN, "addr outside the model window");
        unsafe { (base as *mut u8).add(addr - MODEL_BASE) }
    })
}

/// Readable/writable byte count at a MODEL_BASE-space address for an
/// n-byte access, given the fault window.
fn model_readable(addr: usize, n: usize) -> usize {
    MODEL.with(|m| match m.borrow().fault_off {
        Some(f) => {
            let off = addr - MODEL_BASE;
            f.saturating_sub(off).min(n)
        }
        None => n,
    })
}

unsafe fn copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    let key = model_addr_of(from);
    let readable = model_readable(key, n);
    let off = key - MODEL_BASE;
    BACKING.with(|b| {
        let b = b.borrow();
        for (i, byte) in b[off..off + readable].iter().enumerate() {
            *to.add(i) = *byte;
        }
    });
    n - readable
}

unsafe fn copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    let key = model_addr_of(to);
    let writable = model_readable(key, n);
    let off = key - MODEL_BASE;
    BACKING.with(|b| {
        let mut b = b.borrow_mut();
        b[off..off + writable].copy_from_slice(core::slice::from_raw_parts(from, writable));
    });
    n - writable
}

unsafe fn __copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    copy_from_user(to, from, n)
}

// --- text-faithful mirror of kernel/core/src/arch/riscv64/uaccess.rs ---

/// Mirror of kernel get_user (riscv64/uaccess.rs).
#[inline]
pub unsafe fn get_user<T: Copy>(from: *const T) -> Option<T> {
    let size = core::mem::size_of::<T>();

    if !access_ok(model_addr_of(from), size) {
        return None;
    }

    let mut value: core::mem::MaybeUninit<T> = core::mem::MaybeUninit::uninit();

    let uncopied = copy_from_user(
        value.as_mut_ptr() as *mut u8,
        from as *const u8,
        size,
    );

    if uncopied == 0 {
        Some(value.assume_init())
    } else {
        None
    }
}

/// Mirror of kernel put_user (riscv64/uaccess.rs).
#[inline]
pub unsafe fn put_user<T: Copy>(to: *mut T, value: T) -> bool {
    let size = core::mem::size_of::<T>();

    if !access_ok(model_addr_of(to), size) {
        return false;
    }

    let uncopied = copy_to_user(
        to as *mut u8,
        &value as *const T as *const u8,
        size,
    );

    uncopied == 0
}

/// Mirror of kernel strnlen_user (riscv64/uaccess.rs).
pub unsafe fn strnlen_user(str: *const u8, maxlen: usize) -> usize {
    if maxlen == 0 {
        return 0;
    }

    if !access_ok(model_addr_of(str), 1) {
        return 0;
    }

    let mut len = 0;
    while len < maxlen {
        match get_user(str.add(len) as *const u8) {
            Some(0) => return len + 1, // Include null terminator
            Some(_) => len += 1,
            None => return 0,
        }
    }

    0 // String too long
}

/// Mirror of kernel strncpy_from_user (riscv64/uaccess.rs). The
/// `super::mm::user_addr::USER_END` path is canonicalized to this file's
/// USER_END constant by the sync-checker transform.
pub fn strncpy_from_user<'a>(from: *const u8, max_len: usize, buf: &'a mut [u8]) -> Result<&'a [u8], i64> {
    // EFAULT = 14
    const EFAULT: i64 = 14;

    if from.is_null() {
        return Err(-EFAULT);
    }

    // Verify the pointer itself is in user space.
    if !access_ok(model_addr_of(from), 1) {
        return Err(-EFAULT);
    }

    // Compute max readable bytes as distance from pointer to
    // USER_END. This avoids the old bug where access_ok(from, max_len)
    // failed when from was near the end of user space.
    let addr = model_addr_of(from);
    let user_end = USER_END;
    let max = if addr < user_end {
        user_end - addr
    } else {
        return Err(-EFAULT);
    };
    let limit = core::cmp::min(max_len, buf.len());
    let limit = core::cmp::min(limit, max);

    const CHUNK: usize = 64;
    let mut done: usize = 0;
    while done < limit {
        let want = core::cmp::min(CHUNK, limit - done);
        // SAFETY: buf is `limit` bytes; done+want stays inside it. The
        // raw exception-table copy handles user faults: it returns the
        // bytes NOT copied of this chunk.
        let uncopied = unsafe {
            __copy_from_user(buf.as_mut_ptr().add(done), from.add(done), want)
        };
        let got = want - uncopied;
        if uncopied > 0 {
            // SAFETY: zero the never-written tail so the NUL scan below
            // cannot see stale kernel bytes.
            unsafe {
                core::ptr::write_bytes(buf.as_mut_ptr().add(done + got), 0, uncopied)
            };
        }
        if let Some(rel) = buf[done..done + got].iter().position(|&b| b == 0) {
            return Ok(&buf[..done + rel]);
        }
        if uncopied > 0 {
            // Faulted with no NUL in the readable window so far.
            return Err(-EFAULT);
        }
        done += want;
    }
    // No NUL within `limit` bytes: same shape the old loop returned.
    Ok(&buf[..limit])
}

// ============================================================================
// Model-driven properties for the accessors above
// ============================================================================

/// Backing window for model addresses (any integer keys the model; this
/// window keeps generated addresses realistic and away from USER_END).
const MODEL_BASE: usize = 0x1000_0000;
const MODEL_SPAN: usize = 8192;
/// The real backing allocation: far larger than the window so the
/// string-loop offsets (up to ~256 bytes past the window start) stay
/// in-bounds inside the allocation.
const BACKING_SPAN: usize = 65536;

/// Chunk size of the mirrored strncpy_from_user loop (the fn-local const
/// of the same value is part of the synced kernel text and not addressable
/// from the tests; this is the model-side twin).
const STRN_CHUNK: usize = 64;

/// Generated addresses stay inside the model window (so the real
/// backing pointers stay in-bounds under Miri). Out-of-window rejection
/// paths get their own boundary tests with never-dereferenced pointers
/// (get_user_rejects_out_of_user_space below).
fn model_addr() -> impl proptest::strategy::Strategy<Value = usize> {
    MODEL_BASE..MODEL_BASE + MODEL_SPAN
}

/// get_user returns Some iff the gate passes AND the model can supply the
/// whole value; the returned value round-trips the model bytes.
proptest! {
    #[test]
    fn get_user_gate_and_roundtrip(
        addr in model_addr(),
        val in any::<u64>(),
        fault_kind in 0u8..4,
    ) {
        let size = core::mem::size_of::<u64>();
        backing_set(addr, &val.to_ne_bytes());
        // 0 = no fault; 1 = fault at first byte; 2..3 = fault mid-value
        let fault_at = match fault_kind {
            0 => None,
            1 => Some(addr),
            k => Some(addr + (k as usize - 1)),
        };
        model_setup(fault_at);

        let got = unsafe { get_user::<u64>(real_ptr(addr) as *const u64) };
        let fully_readable = match fault_at {
            None => true,
            Some(f) => addr + size <= f,
        };
        let expect_some = access_ok(addr, size) && fully_readable;
        prop_assert_eq!(got.is_some(), expect_some);
        if let Some(v) = got {
            prop_assert_eq!(v, val);
        }
    }
}

/// put_user returns true iff the gate passes AND no fault; on success the
/// model holds the value's native-endian bytes.
proptest! {
    #[test]
    fn put_user_gate_and_write(
        addr in model_addr(),
        val in any::<u32>(),
        fault_kind in 0u8..4,
    ) {
        let size = core::mem::size_of::<u32>();
        let fault_at = match fault_kind {
            0 => None,
            1 => Some(addr),
            k => Some(addr + (k as usize - 1)),
        };
        model_setup(fault_at);

        let ok = unsafe { put_user::<u32>(real_ptr_mut(addr) as *mut u32, val) };
        let fully_writable = match fault_at {
            None => true,
            Some(f) => addr + size <= f,
        };
        let expect_ok = access_ok(addr, size) && fully_writable;
        prop_assert_eq!(ok, expect_ok);
        if ok {
            // read the stored bytes out of the model first: the
            // prop_assert macros must run outside thread-local closures
            let stored: Vec<u8> = backing_get(addr, size);
            let bytes = val.to_ne_bytes();
            for (i, b) in stored.iter().enumerate() {
                prop_assert_eq!(*b, bytes[i], "model byte {} after put_user", i);
            }
        }
    }
}

/// strnlen_user: result is 0 or (NUL index + 1) within maxlen; never
/// exceeds maxlen; reports 0 on fault before the terminator.
proptest! {
    #[test]
    fn strnlen_bounded_scan(
        nul_at in 0usize..200usize,
        maxlen in 1usize..256usize,
        fault_kind in 0u8..4, // 0 none / 1 at the NUL / 2 before the NUL / 3 after
    ) {
        let addr = MODEL_BASE;
        let mut content = vec![0u8; MODEL_SPAN];
        for i in 0..nul_at {
            content[i] = b'a';
        }
        content[nul_at] = 0u8;
        backing_set(addr, &content);
        let fault_at = match fault_kind {
            0 => None,
            1 => Some(addr + nul_at),
            2 if nul_at >= 2 => Some(addr + nul_at / 2),
            _ => Some(addr + nul_at + 7),
        };
        model_setup(fault_at);

        let got = unsafe { strnlen_user(real_ptr(addr), maxlen) };
        // the NUL is readable iff no fault covers it
        let nul_readable = match fault_at {
            None => true,
            Some(f) => f > addr + nul_at,
        };
        if !nul_readable {
            prop_assert_eq!(got, 0, "fault before the terminator must report 0");
        } else if nul_at < maxlen {
            prop_assert_eq!(got, nul_at + 1);
        } else {
            // no NUL within maxlen: too-long string reports 0
            prop_assert_eq!(got, 0);
        }
        prop_assert!(got <= maxlen);
    }
}

/// strncpy_from_user: exact user bytes before the terminator, NUL pinned
/// at the return length, and — the info-leak pin — a faulted tail is
/// ZEROED: the 0xA5 sentinel may never survive beyond the readable
/// extent. This pins the chunk loop's zero-fill discipline.
proptest! {
    #[test]
    fn strncpy_no_stale_tail(
        nul_at in 0usize..150usize,
        max_len in 1usize..256usize,
        fault_after_nul in 0usize..200usize,
    ) {
        const BUF_LEN: usize = 256;
        let addr = MODEL_BASE;
        let mut content = vec![0u8; MODEL_SPAN];
        for i in 0..nul_at {
            content[i] = b'a';
        }
        content[nul_at] = 0u8;
        backing_set(addr, &content);
        // fault strictly AFTER the terminator: the scan must still find
        // the NUL (kernel comment: "the scan either finds the NUL first")
        let fault_at = Some(addr + nul_at + 1 + fault_after_nul);
        model_setup(fault_at);

        let mut buf = [0xA5u8; BUF_LEN];
        // map to a length first: the returned slice borrows `buf`, and the
        // assertions below need buf free of that borrow
        let result = strncpy_from_user(real_ptr(addr), max_len, &mut buf)
            .map(|s| s.len());

        // limit mirrors the kernel computation
        let limit = max_len.min(BUF_LEN);
        // readable extent in buf coordinates (fault is strictly after the NUL)
        let fault_off = fault_at.unwrap() - addr;
        match result {
            Ok(ret_len) => {
                if ret_len < limit {
                    // NUL-terminated shape: the terminator is at nul_at
                    prop_assert_eq!(ret_len, nul_at, "return stops at the NUL");
                    prop_assert_eq!(buf[ret_len], 0u8);
                }
                // exact user bytes before the terminator
                for i in 0..ret_len {
                    prop_assert_eq!(buf[i], b'a', "prefix byte {}", i);
                }
                // faulted tail within the LAST EXECUTED chunk must be
                // zero, not the 0xA5 sentinel (stale-kernel-byte
                // disclosure pin). The kernel zero-fills only the faulted
                // chunk's tail; chunks the loop never reached keep their
                // caller-provided contents, so the check range stops at
                // the executed-chunk end.
                let chunk_of_ret = ret_len / STRN_CHUNK * STRN_CHUNK;
                let executed_end =
                    chunk_of_ret + STRN_CHUNK.min(limit.saturating_sub(chunk_of_ret));
                for i in (ret_len + 1)..limit.min(executed_end) {
                    if i >= fault_off {
                        prop_assert_eq!(buf[i], 0u8, "stale byte at {} in executed chunk", i);
                    }
                }
            }
            Err(e) => {
                prop_assert_eq!(e, -14i64);
                // Err only when no NUL lies within the readable limit
                prop_assert!(nul_at >= limit, "NUL before the limit must succeed");
                prop_assert!(fault_off < limit, "fault beyond the limit never fails");
            }
        }
    }
}

/// Boundary pins carried over from the 20-byte-name regression: a short
/// name costs exactly one chunk and the returned slice excludes the NUL.
#[test]
fn strncpy_short_name_single_chunk() {
    let addr = MODEL_BASE;
    let name = b"/etc/passwd";
    let mut content = vec![0u8; MODEL_SPAN];
    content[..name.len()].copy_from_slice(name);
    content[name.len()] = 0u8;
    backing_set(addr, &content);
    model_setup(None);

    let mut buf = [0u8; 128];
    let got = strncpy_from_user(real_ptr(addr), 4096, &mut buf).unwrap();
    assert_eq!(got, name);
}

/// Rejection boundary pins: addresses above the user ceiling (and
/// wrapping ranges) are rejected before any access. The pointers here
/// are integer-cast and never dereferenced nor offset — legal to
/// construct, and the gate must bounce them without touching memory.
#[test]
fn get_user_rejects_out_of_user_space() {
    // strictly above USER_END: rejected for every size
    assert!(unsafe { get_user::<u64>(USER_END.wrapping_add(64) as *const u64) }.is_none());
    // wrapping range: addr+size overflows
    assert!(unsafe { get_user::<u64>(usize::MAX as *const u64) }.is_none());
    // one byte at exactly USER_END overruns the ceiling (end =
    // USER_END + 1 > USER_END): the gate rejects before any access.
    assert!(unsafe { get_user::<u8>(USER_END as *const u8) }.is_none());
}
