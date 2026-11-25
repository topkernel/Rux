//! Host-side Miri battery over the kernel's safety primitives
//! (unsafe-5000-plan Layer 2.1, the "geology check").
//!
//! The primitive sources are included VERBATIM (`include!` of the
//! kernel/core/src/safety/*.rs files — no copies to drift). Two host
//! shims make that possible:
//! - `arch::mm` provides an identity `virt_to_phys` for KPtr::phys()
//!   (the kernel's real one walks the kernel page table — meaningless
//!   on a host; the identity twin exercises the same call shape);
//! - everything frame.rs/dma.rs (kernel stacks, physical DMA
//! segments) is NOT included: Miri cannot see those memories; those
//! primitives stay covered by their in-tree contracts (INV ids) and
//! the future Kani work (plan Layer 2.2).
//!
//! Acceptance (plan): zero UB across this suite — any hit is
//! P0-below-the-P0s (it invalidates every SAFETY contract built on
//! the primitive).

/// Host shim: the identity address translation ptr.rs's `phys()`
/// calls. On the kernel this walks the real page tables; here the
/// pointer IS the address, which is all the Miri battery needs (the
/// fn under test is KPtr's call discipline, not the MMU).
pub mod arch {
    pub mod mm {
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub struct VirtAddr(pub u64);
        impl VirtAddr {
            pub fn new(v: u64) -> Self {
                VirtAddr(v)
            }
        }
        pub fn virt_to_phys(v: VirtAddr) -> VirtAddr {
            v
        }
    }

    /// Host shim: the generic no-MPK fallback, same shape as the
    /// kernel's non-x86_64 twin (kernel/core/src/arch/mod.rs). dma.rs's
    /// guarded-window path calls these; with guard_on() == false the
    /// zone_open/zone_close pair is never taken, which is exactly the
    /// unguarded-host semantics Miri should exercise.
    pub mod mpk {
        #[inline]
        pub fn guard_on() -> bool {
            false
        }
        #[inline]
        pub fn zone_open() -> u32 {
            0
        }
        #[inline]
        pub fn zone_close(_prev: u32) {}
        pub fn tag_kernel_pages(_va: usize, _len: usize) -> usize {
            0
        }
        pub fn untag_kernel_pages(_va: usize, _len: usize) {
        }
    }
}

// std links `alloc` implicitly; the kernel sources under test use
// alloc::alloc (KPtr storage) - make the extern explicit.
extern crate alloc;

#[path = "../../core/src/safety/bytes.rs"]
pub mod bytes;
#[path = "../../core/src/safety/call.rs"]
pub mod call;
#[path = "../../core/src/safety/mmio.rs"]
pub mod mmio;
#[path = "../../core/src/safety/ptr.rs"]
pub mod ptr;
#[path = "../../core/src/safety/ring.rs"]
pub mod ring;
#[path = "../../core/src/safety/slot.rs"]
pub mod slot;

#[path = "../../core/src/safety/dma.rs"]
pub mod dma;
#[path = "../../core/src/safety/frame.rs"]
pub mod frame;

#[cfg(kani)]
mod kani;
