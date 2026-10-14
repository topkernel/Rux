//! aarch64 uaccess — bring-up skeleton.
//!
//! Interface only; no implementation. Porting notes: access_ok + copy_to/from_user via kernel uaccess window (arm64 SUM equivalent: uaccess enable/disable via PSTATE.PAN).

pub fn placeholder() {
    unimplemented!("aarch64 backend bring-up: uaccess")
}
