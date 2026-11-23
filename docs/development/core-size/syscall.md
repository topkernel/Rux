# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# syscall — hot 0 lines (stay) / cold 2706 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| syscall/dispatch.rs | `native_table_call` | 26 | 23 |  |
| syscall/dispatch.rs | `syscall_register_tables` | 48 | 20 |  |
| syscall/dispatch.rs | `table_lookup` | 67 | 21 |  |
| syscall/dispatch.rs | `syscall_get_nr` | 87 | 7 |  |
| syscall/dispatch.rs | `syscall_get_arguments` | 93 | 8 |  |
| syscall/dispatch.rs | `syscall_get_return` | 100 | 26 |  |
| syscall/dispatch.rs | `new` | 125 | 10 |  |
| syscall/dispatch.rs | `record` | 134 | 16 |  |
| syscall/dispatch.rs | `syscall_set_return_value` | 149 | 9 |  |
| syscall/dispatch.rs | `strace1_line` | 157 | 111 |  |
| syscall/dispatch.rs | `lastsys_latch_arm` | 267 | 64 |  |
| syscall/dispatch.rs | `syscall_handler_inner` | 330 | 901 |  |
| syscall/dispatch.rs | `translate` | 1230 | 374 |  |
| syscall/dispatch.rs | `native_enosys_hole` | 1603 | 39 |  |
| syscall/edges.rs | `list_first` | 56 | 10 |  |
| syscall/edges.rs | `list_next` | 65 | 10 |  |
| syscall/edges.rs | `list_entry` | 74 | 13 |  |
| syscall/edges.rs | `list_entry_mut` | 86 | 14 |  |
| syscall/edges.rs | `list_link_tail` | 99 | 11 |  |
| syscall/edges.rs | `list_unlink` | 109 | 36 |  |
| syscall/edges.rs | `boxed_ref` | 144 | 13 |  |
| syscall/edges.rs | `boxed_mut` | 156 | 12 |  |
| syscall/edges.rs | `boxed_drop` | 167 | 18 |  |
| syscall/edges.rs | `task_park_unwind` | 184 | 35 |  |
| syscall/edges.rs | `pidfd_data_of` | 218 | 18 |  |
| syscall/edges.rs | `file_inode_set_pre_publish` | 235 | 25 |  |
| syscall/edges.rs | `arch_map_page` | 259 | 27 |  |
| syscall/edges.rs | `unmap_user_leaf_locked` | 285 | 18 |  |
| syscall/edges.rs | `copy_page_linear` | 302 | 29 |  |
| syscall/edges.rs | `page_desc_of` | 330 | 22 |  |
| syscall/edges.rs | `mprotect_rewrite_leaf_sv39` | 351 | 90 |  |
| syscall/edges.rs | `pt_walk_writable_phys` | 440 | 57 |  |
| syscall/edges.rs | `file_write_page` | 496 | 23 |  |
| syscall/edges.rs | `arch_map_user_page` | 518 | 23 |  |
| syscall/edges.rs | `slice_of_kernel_bytes` | 540 | 6 |  |
| syscall/mod.rs | `seam_register_default` | 25 | 267 |  |
| syscall/mod.rs | `new` | 291 | 5 |  |
| syscall/mod.rs | `set` | 295 | 8 |  |
| syscall/mod.rs | `clear` | 302 | 8 |  |
| syscall/mod.rs | `is_set` | 309 | 10 |  |
| syscall/mod.rs | `zero` | 318 | 12 |  |
| syscall/seccomp.rs | `seccomp_syscall_gate` | 112 | 48 |  |
| syscall/seccomp.rs | `seccomp_run` | 159 | 68 |  |
| syscall/support.rs | `clamp_rw_count` | 18 | 62 |  |
| syscall/support.rs | `syscall_register_hooks` | 79 | 6 |  |
| syscall/support.rs | `hooks` | 84 | 6 |  |
| syscall/support.rs | `epoll_interest_snapshot` | 89 | 3 |  |
| syscall/support.rs | `timerfd_wheel_audit` | 91 | 3 |  |
| syscall/support.rs | `pid1_epoll_interest_dump` | 93 | 3 |  |
| syscall/support.rs | `kernel_random_bytes` | 95 | 3 |  |
| syscall/support.rs | `oh67_fd_ready` | 97 | 3 |  |
| syscall/support.rs | `jiffies_covering_deadline` | 99 | 3 |  |
| syscall/support.rs | `deadline_ns_to_hres_ticks` | 101 | 3 |  |
| syscall/support.rs | `timerfd_expire_notify` | 103 | 3 |  |
| syscall/support.rs | `param_stall_dump` | 105 | 3 |  |
| syscall/support.rs | `is_eventfd_ops` | 107 | 3 |  |
| syscall/support.rs | `syscall_eio_line` | 109 | 3 |  |
| syscall/support.rs | `epoll_dump_for_file` | 111 | 17 |  |
| syscall/support.rs | `new` | 127 | 17 |  |
