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
//! Not here: fs/file_hooks.rs and fs/notify_hooks.rs hold no table (they
//! forward to rows of `fs`). Every table is in this directory: 22 in 14
//! files.

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
