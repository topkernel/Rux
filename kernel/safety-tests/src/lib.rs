//! Host-side Miri battery over the kernel's safety primitives
//! (unsafe-5000-plan Layer 2.1, the "geology check").
//!
//! The primitive sources are included VERBATIM (`include!` of the
//! kernel/src/safety/*.rs files — no copies to drift). Two host
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
}

// std links `alloc` implicitly; the kernel sources under test use
// alloc::alloc (KPtr storage) - make the extern explicit.
extern crate alloc;

#[path = "../../src/safety/bytes.rs"]
pub mod bytes;
#[path = "../../src/safety/call.rs"]
pub mod call;
#[path = "../../src/safety/mmio.rs"]
pub mod mmio;
#[path = "../../src/safety/ptr.rs"]
pub mod ptr;
#[path = "../../src/safety/ring.rs"]
pub mod ring;
#[path = "../../src/safety/slot.rs"]
pub mod slot;

#[path = "../../src/safety/dma.rs"]
pub mod dma;
#[path = "../../src/safety/frame.rs"]
pub mod frame;

#[cfg(kani)]
mod kani;
