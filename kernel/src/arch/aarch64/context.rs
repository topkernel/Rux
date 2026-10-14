//! aarch64 context — bring-up skeleton.
//!
//! Interface only; no implementation. Porting notes: context_switch (callee-saved x19..x28, sp, lr via a register-frame swap) + FPU/SIMD save-restore.

pub fn placeholder() {
    unimplemented!("aarch64 backend bring-up: context")
}
