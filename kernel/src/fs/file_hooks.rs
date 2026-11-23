//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Rung-3 OPS-HOOK SEAM: the framework-side per-File operation hooks.
//! pipe/fifo/memfd dispatch through the FsHooks registration table
//! (the rux-fs crate registers at boot — the trampolines became
//! slots at the flip); char_dev stays framework-side (direct).

/// FIONREAD/stat fallback for a pipe File (None = not a pipe).
pub(crate) fn pipe_stat(file: &crate::fs::file::File, stat: &mut crate::fs::Stat) -> Option<i32> {
    (crate::fs::fs_hooks::FS_HOOKS.get().unwrap_or(&crate::fs::fs_hooks::FsHooks::EMPTY).pipe_stat)(file, stat)
}

/// F_GETPIPE_SZ for a pipe File (None = not a pipe).
pub(crate) fn pipe_get_sz(file: &crate::fs::file::File) -> Option<usize> {
    (crate::fs::fs_hooks::FS_HOOKS.get().unwrap_or(&crate::fs::fs_hooks::FsHooks::EMPTY).pipe_get_sz)(file)
}

/// F_SETPIPE_SZ for a pipe File.
pub(crate) fn pipe_set_sz(
    file: &crate::fs::file::File,
    arg: usize,
) -> Result<usize, i32> {
    (crate::fs::fs_hooks::FS_HOOKS.get().unwrap_or(&crate::fs::fs_hooks::FsHooks::EMPTY).pipe_set_sz)(file, arg)
}

/// O_ASYNC SIGIO registration for a pipe File (no-op for non-pipes).
pub(crate) fn oasync_register(file: &crate::fs::file::File, fd: i32) {
    (crate::fs::fs_hooks::FS_HOOKS.get().unwrap_or(&crate::fs::fs_hooks::FsHooks::EMPTY).pipe_oasync_register)(file, fd)
}
pub(crate) fn oasync_unregister(file_id: u64) {
    (crate::fs::fs_hooks::FS_HOOKS.get().unwrap_or(&crate::fs::fs_hooks::FsHooks::EMPTY).pipe_oasync_unregister)(file_id)
}

/// stat fallback for a char-device File (the char_dev leaf lives in the
/// rux-fs crate; dispatch through the registered row).
pub(crate) fn char_dev_stat(file: &crate::fs::file::File, stat: &mut crate::fs::Stat) -> Option<i32> {
    (crate::fs::fs_hooks::hooks().char_dev_file_stat)(file, stat)
}

// ===== fifo + memfd (slot dispatch) =====

/// FIFO teardown on inode unlink.
pub(crate) fn fifo_forget(fs_id: u64, ino: u64) {
    (crate::fs::fs_hooks::FS_HOOKS.get().unwrap_or(&crate::fs::fs_hooks::FsHooks::EMPTY).fifo_forget)(fs_id, ino)
}

/// FIFO special-open: peer resolution + open, one slot call.
pub(crate) fn fifo_open(
    fs_id: u64,
    ino: u64,
    flags: u32,
) -> Result<alloc::sync::Arc<crate::fs::file::File>, i32> {
    (crate::fs::fs_hooks::FS_HOOKS.get().unwrap_or(&crate::fs::fs_hooks::FsHooks::EMPTY).fifo_open)(fs_id, ino, flags)
}

/// memfd seals: add (F_ADD_SEALS) / query (F_GET_SEALS).
pub(crate) fn memfd_add_seals(file: &crate::fs::file::File, seals: u32) -> Result<(), i32> {
    (crate::fs::fs_hooks::FS_HOOKS.get().unwrap_or(&crate::fs::fs_hooks::FsHooks::EMPTY).memfd_add_seals)(file, seals)
}
pub(crate) fn memfd_get_seals(file: &crate::fs::file::File) -> Result<u32, i32> {
    (crate::fs::fs_hooks::FS_HOOKS.get().unwrap_or(&crate::fs::fs_hooks::FsHooks::EMPTY).memfd_get_seals)(file)
}
