//! aarch64 boot — bring-up skeleton.
//!
//! Interface only; no implementation. Porting notes: boot protocol (device-tree from QEMU -M virt, kernel image at 0x40080000, PSCI conduit detection).

pub fn placeholder() {
    unimplemented!("aarch64 backend bring-up: boot")
}
