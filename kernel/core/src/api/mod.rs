//! The interface tables (seed-core-design 2c.4): the function-pointer
//! tables through which the core reaches a service crate. The core
//! defines each table's signature here; a service crate fills a table
//! by registering at boot, so these files are the service's entire view
//! of the core. One file per subsystem -- what a service crate can do
//! to the core is this directory listing.
//!
//! Tables that are filled at different boot points stay separate
//! structs; nothing in this directory merges or fills a table.
//!
//! Not here: fs/file_hooks.rs, fs/notify_hooks.rs and fs/lock_hooks.rs
//! stayed in fs/ when this directory was made. The first two hold no
//! table (they forward to rows of `fs`); fs/lock_hooks.rs holds one,
//! LocksHooks (nine rows), so this listing is one table short.

pub mod arch;
pub mod boot;
pub mod debug;
pub mod drivers;
pub mod ext4;
pub mod fs;
pub mod ipc;
pub mod mm;
pub mod net;
pub mod process;
pub mod sched;
pub mod signal;
pub mod sync;
pub mod timer;
