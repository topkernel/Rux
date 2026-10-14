//! aarch64 process — bring-up skeleton.
//!
//! Interface only; no implementation. Porting notes: copy_thread/thread setup: set pstate=EL0, pc=entry, sp=user sp; ret_from_fork frame.

pub fn placeholder() {
    unimplemented!("aarch64 backend bring-up: process")
}
