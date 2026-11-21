//! The Miri UB battery over the safety primitives. Every test is
//! written to FAIL under Miri if the primitive's contract is wrong —
//! allocation/drop cycles, aliasing windows, bounds math, spsc
//! ordering, publish-once.

use kernel_safety_tests::{bytes, mmio, ptr, ring, slot};

// ============================ CopyBytes ============================

#[test]
fn bytes_roundtrip_pod() {
    #[derive(Clone, Copy, PartialEq, Debug)]
    #[repr(C)]
    struct Wire {
        a: u32,
        b: u16,
        c: u8,
    }
    let v = Wire { a: 0xdead_beef, b: 0xcafe, c: 7 };
    let raw = bytes::bytes_of(&v);
    assert_eq!(raw.len(), core::mem::size_of::<Wire>());
    let back: Wire = bytes::from_bytes(raw).unwrap();
    assert_eq!(back, v);
}

#[test]
fn bytes_read_at_write_at_unaligned_offsets() {
    let mut buf = [0u8; 16];
    bytes::write_at(&mut buf, 3, 0x1234_5678u32).unwrap();
    assert_eq!(bytes::read_at::<u32>(&buf, 3), Some(0x1234_5678));
    // short-buffer refusals, not OOB
    assert!(bytes::read_at::<u32>(&buf, 13).is_none());
    assert!(bytes::write_at(&mut buf, 14, 9u32).is_none());
    assert!(bytes::from_bytes::<u64>(&buf[..7]).is_none());
}

// ============================ KPtr ============================

#[test]
fn kptr_alloc_deref_drop_exactly_once() {
    let p = ptr::KPtr::new(41u64).unwrap();
    assert_eq!(*p, 41);
    // move through a scope: exactly one Drop must run — a second Drop
    // is a Miri-reported double free right here
    {
        let p2 = p;
        assert_eq!(*p2, 41);
    }
}

#[test]
fn kptr_volatile_rw_cycle() {
    let p = ptr::KPtr::new([0u8; 8]).unwrap();
    p.write_volatile([1u8; 8]);
    let snapshot: [u8; 8] = p.read_volatile();
    assert_eq!(snapshot, [1u8; 8]);
}

#[test]
fn kptr_phys_identity_under_shim() {
    let p = ptr::KPtr::new(5u32).unwrap();
    // host shim: identity translation; exercises the call discipline
    assert_eq!(p.phys(), p.as_ptr() as u64);
}

// ============================ SpscRing ============================

#[test]
fn spsc_ring_fill_drain_wrap() {
    let r: ring::SpscRing<u32, 8> = ring::SpscRing::new();
    let mut popped = 0usize;
    // interleave puts and gets across the wrap boundary: a full round
    // drops the overflow value by contract (INV-RING-2), so put 5 of
    // 8 and drain all 5 each time; a lost/duplicated slot is a Miri-
    // visible aliasing or leak class.
    for round in 0..100u32 {
        for i in 0..5u32 {
            r.put(round * 5 + i);
        }
        for _ in 0..5 {
            if r.get().is_some() {
                popped += 1;
            }
        }
    }
    assert_eq!(popped, 500);
    assert!(r.get().is_none(), "empty after balanced drain");
}

#[test]
fn spsc_ring_overflow_drops_not_wedges() {
    // Classic ring-buffer capacity semantics: usable slots = N-1 (the
    // `next == head` full check sacrifices one slot - INV-RING-2's
    // drop-on-full). N MUST be a power of two (the MASK const is 0
    // otherwise and the ring degenerates - battery finding #1, see
    // the lane report). Capacity 4 holds 3.
    let r: ring::SpscRing<u8, 4> = ring::SpscRing::new();
    r.put(1);
    r.put(2);
    r.put(3);
    r.put(4); // overflow: dropped by contract
    assert_eq!(r.get(), Some(1));
    assert_eq!(r.get(), Some(2));
    assert_eq!(r.get(), Some(3));
    assert_eq!(r.get(), None);
}

// ============================ OnceSlot ============================

#[test]
fn once_slot_publish_once() {
    let s: slot::OnceSlot<u64> = slot::OnceSlot::new();
    assert!(s.get().is_none());
    assert!(s.publish(1));
    assert!(!s.publish(2));
    assert_eq!(*s.get().unwrap(), 1);
}

// ============================ MmioRegion ============================

#[test]
fn mmio_window_in_bounds_ops() {
    // A region over a REAL host allocation: Miri can see it, so any
    // window math error is a caught OOB, not a silent one. The backing
    // is u32-aligned (battery finding #2: the API trusts natural
    // alignment as an unenforced precondition - SAFETY note in
    // read_u32 - so the battery exercises the documented contract,
    // and the misaligned-base UB is REPORTED, not papered over).
    let mut backing = [0u32; 4];
    let region = mmio::MmioRegion::new(backing.as_mut_ptr() as usize, 16);
    region.write_u32(4, 0xaabb_ccdd);
    assert_eq!(region.read_u32(4), 0xaabb_ccdd);
    region.write_u8(15, 0x55);
    assert_eq!(region.read_u8(15), 0x55);
    // the writes landed in the backing store
    assert_eq!(backing[1], 0xaabb_ccdd);
    assert_eq!((backing[3] >> 24) as u8, 0x55);
    let _ = backing;
}

#[test]
#[should_panic(expected = "out of window")]
fn mmio_window_oob_panics_not_ub() {
    // Aligned backing AND aligned offset that still exceeds the window
    // (8+4 > 8 bytes): the WINDOW gate fires — the loud panic, never an
    // OOB read. (Misaligned pairs hit the alignment assert first since
    // the Layer-2.1 finding #2 was enforced; that arm is covered by the
    // Kani should_panic harnesses over all offsets.)
    let backing = [0u32; 2];
    let region = mmio::MmioRegion::new(backing.as_ptr() as usize, 8);
    let _ = region.read_u32(8);
}
