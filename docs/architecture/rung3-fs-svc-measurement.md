# Rung 3 reverse-surface measurement — rux-fs-svc (2026-11-23, L5)

Method per crate-plan §3 step 1 (measure → decompose by class → price).
Two directions measured: LEAF→framework (consumption; stays legal via
the framework crate's pub API) and FRAMEWORK→leaf (the reverse surface
that must sever/relocate). NO code changed.

## The reverse surface: 128 framework→leaf references, 4 files

| Framework file | Refs | Leaf items consumed | Family |
|---|---:|---|---|
| vfs.rs | 63 | procfs:4 (pid dir ops, NS/FD link offsets), inotify:4 (notify/alloc_cookie/events), fifo:3 (open/forget/peer), locks:3 (lock kinds), pipe:4 (stat/sz/oasync), char_dev/memfd:2 | notify+lookup |
| file.rs | 25 | locks:4 (flock/ofd/open-register), inotify:3 (notify close/events), pipe:1 (oasync) | lifecycle notify |
| mount.rs | 39 | tmpfs:22 (FS_TYPE + INodeOps tables incl. devpts/configfs/mqueue aliases), mount init fns for procfs/devfs/sysfs/cgroup/debugfs/tracefs/selinuxfs:17 | the mount table |
| superblock.rs/bio.rs/page_cache.rs/mod.rs/inode.rs | 0 | — | CLEAN |

## Family decomposition + predicted relocation count

| Family | Refs | Shape | Predicted relocations |
|---|---:|---|---|
| inotify notify fanout (vfs+file) | 7 | framework calls notify_file/events/close on every inode/file lifecycle event | 0 relocations: notify hook TABLE in core (register once at inotify init); leaf calls down, framework calls through the hook slot |
| locks flock/ofd/open-register (file+vfs) | 22 | file close/release paths call lock release + open registry | 2 moves: LockOwner registry + release fns DOWN into core (they are pure bookkeeping on File/Inode); kinds stay leaf-side constants re-exported |
| pipe stat/sz/oasync (vfs+file) | 15 | F_GETPIPE_SZ/stat/oasync on close | 0 relocations: pipe ops table already registered on the File; the 3 cross-calls become a FileOps optional hook (get_sz/stat already exist as ops-shape) |
| fifo open/forget/peer (vfs) | 3 | special open path | 1 move: fifo_open_file DOWN (it is an inode-shape helper) or FIFO-ops registration on inode type |
| procfs pid-dir + NS/FD offsets (vfs) | 19 | /proc self-links | 2 moves: offset constants DOWN; pid-dir lookup via INodeOps on the procfs root (already table-shaped) |
| tmpfs tables + aliases (mount) | 22 | mount init dispatch by fs name | 0 relocations: THE ENTRY SEAM — mount.rs's per-name dispatch becomes a registration table (name → mount fn + INodeOps), each leaf registers at init (same shape as FileSystemType::new already is) |
| other-leaf mount inits (mount) | 17 | same | 0 (same interface) |
| memfd seals, char_dev stat, cgroup/misc | 9 | scattered single calls | 3 moves: seals fns DOWN to file.rs core (memfd-generic), char_dev stat via FileOps hook, cgroup trivial |

**Predicted relocation total: ~8 moves + 2 hook tables (notify, mount
registration) + 1 ops-hook extension (pipe).** The 128 references
decompose into 3 interface mechanisms — nothing un-decomposable found.
Rung 3 is PRICED and proceeds.

## Consumption side (leaf→framework, for the crate API surface)

Dominant framework items by leaf (top-3 each): procfs inode=42/VFS=11/file=9;
devfs file=29/VFS=26/inode=18; tmpfs VFS=15/inode=11/page_cache=5; pipe
file=19/VFS=8/uaccess=4; pty devfs=6(cross-leaf!)/VFS=5/file=5; tty
CopyBytes=24; io_uring file=14/MmioRegion=9/uaccess=4. CROSS-LEAF refs
flagged for the design: pty→devfs=6, tmpfs→devfs=1, procfs→ext4=2,
devfs→char_dev=1+char_dev→console — these need either a shared
"devfs-svc" sub-crate or moving the shared item down (pty's devfs calls
are registration-shape; procfs→ext4 is readlink through generic file ops).

## Entry interfaces (framework→service, decided not by fiat)
1. MOUNT: the name→(mount_fn, INodeOps) registration table (replaces
   mount.rs's hardcoded dispatch) — same shape as FileSystemType today.
2. LIFECYCLE NOTIFY: inotify hook table registered at inotify init.
3. FILE-OPS HOOKS: pipe sz/stat/oasync + char_dev stat as optional
   FileOps/INodeOps slots (already the pattern).
