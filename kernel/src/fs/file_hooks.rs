//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Rung-3 OPS-HOOK SEAM: the framework-side per-File operation hooks
//! (pipe size/stat/O-async + char-device stat). vfs.rs and file.rs call
//! through THIS table; the pipe and char_dev leaves register their
//! trampolines. Same model as notify_hooks (seam 2) — at the crate
//! flip the trampolines become registration slots.

/// FIONREAD/stat fallback for a pipe File (None = not a pipe).
pub(crate) fn pipe_stat(file: &crate::fs::file::File, stat: &mut crate::fs::Stat) -> Option<i32> {
    crate::fs::pipe::pipe_file_stat(file, stat)
}

/// F_GETPIPE_SZ / F_SETPIPE_SZ for a pipe File (None = not a pipe).
pub(crate) fn pipe_get_sz(file: &crate::fs::file::File) -> Option<usize> {
    crate::fs::pipe::pipe_get_sz(file)
}
pub(crate) fn pipe_set_sz(
    file: &crate::fs::file::File,
    arg: usize,
) -> Result<usize, i32> {
    crate::fs::pipe::pipe_set_sz(file, arg)
}

/// O_ASYNC SIGIO registration for a pipe File (no-op for non-pipes).
pub(crate) fn oasync_register(file: &crate::fs::file::File, fd: i32) {
    crate::fs::pipe::oasync_register_pipe(file, fd);
}
pub(crate) fn oasync_unregister(file_id: u64) {
    crate::fs::pipe::oasync_unregister_file(file_id);
}

/// stat fallback for a char-device File (None = not a console device).
pub(crate) fn char_dev_stat(file: &crate::fs::file::File, stat: &mut crate::fs::Stat) -> Option<i32> {
    crate::fs::char_dev::char_dev_file_stat(file, stat)
}

// ===== fifo + memfd hooks (the relocation seam's remaining families) =====

/// FIFO teardown on inode unlink (fifo module's forget).
pub(crate) fn fifo_forget(fs_id: u64, ino: u64) {
    crate::fs::fifo::forget(fs_id, ino);
}

/// FIFO peer resolution (the special-open path's first half).
pub(crate) fn fifo_peer(fs_id: u64, ino: u64) -> alloc::sync::Arc<crate::fs::fifo::FifoPeer> {
    crate::fs::fifo::peer_for(fs_id, ino)
}

/// FIFO open on the resolved peer (the second half).
pub(crate) fn fifo_open_file(
    peer: &alloc::sync::Arc<crate::fs::fifo::FifoPeer>,
    flags: u32,
) -> Result<alloc::sync::Arc<crate::fs::file::File>, i32> {
    crate::fs::fifo::fifo_open_file(peer, flags)
}

/// memfd seals: add (F_ADD_SEALS) / query (F_GET_SEALS).
pub(crate) fn memfd_add_seals(file: &crate::fs::file::File, seals: u32) -> Result<(), i32> {
    crate::fs::memfd::add_seals(file, seals)
}
pub(crate) fn memfd_get_seals(file: &crate::fs::file::File) -> Result<u32, i32> {
    crate::fs::memfd::get_seals(file)
}
