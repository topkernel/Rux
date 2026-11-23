# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# syscall — hot 201 lines (stay) / cold 2446 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| syscall/dispatch.rs | `native_table_call` | 26 | 22 |  |
| syscall/dispatch.rs | `syscall_register_tables` | 48 | 19 |  |
| syscall/dispatch.rs | `table_lookup` | 67 | 20 |  |
| syscall/dispatch.rs | `syscall_get_nr` | 87 | 6 |  |
| syscall/dispatch.rs | `syscall_get_arguments` | 93 | 7 |  |
| syscall/dispatch.rs | `syscall_get_return` | 100 | 25 |  |
| syscall/dispatch.rs | `new` | 125 | 9 | yes |
| syscall/dispatch.rs | `record` | 134 | 15 | yes |
| syscall/dispatch.rs | `syscall_set_return_value` | 149 | 8 |  |
| syscall/dispatch.rs | `strace1_line` | 157 | 110 |  |
| syscall/dispatch.rs | `lastsys_latch_arm` | 267 | 63 | yes |
| syscall/dispatch.rs | `syscall_handler_inner` | 330 | 900 |  |
| syscall/dispatch.rs | `translate` | 1230 | 373 |  |
| syscall/dispatch.rs | `native_enosys_hole` | 1603 | 38 |  |
| syscall/edges.rs | `list_first` | 56 | 9 |  |
| syscall/edges.rs | `list_next` | 65 | 9 | yes |
| syscall/edges.rs | `list_entry` | 74 | 12 |  |
| syscall/edges.rs | `list_entry_mut` | 86 | 13 |  |
| syscall/edges.rs | `list_link_tail` | 99 | 10 |  |
| syscall/edges.rs | `list_unlink` | 109 | 35 |  |
| syscall/edges.rs | `boxed_ref` | 144 | 12 |  |
| syscall/edges.rs | `boxed_mut` | 156 | 11 | yes |
| syscall/edges.rs | `boxed_drop` | 167 | 17 |  |
| syscall/edges.rs | `task_park_unwind` | 184 | 34 |  |
| syscall/edges.rs | `pidfd_data_of` | 218 | 17 |  |
| syscall/edges.rs | `file_inode_set_pre_publish` | 235 | 24 |  |
| syscall/edges.rs | `arch_map_page` | 259 | 26 |  |
| syscall/edges.rs | `unmap_user_leaf_locked` | 285 | 17 |  |
| syscall/edges.rs | `copy_page_linear` | 302 | 28 |  |
| syscall/edges.rs | `page_desc_of` | 330 | 21 |  |
| syscall/edges.rs | `mprotect_rewrite_leaf_sv39` | 351 | 89 |  |
| syscall/edges.rs | `pt_walk_writable_phys` | 440 | 56 |  |
| syscall/edges.rs | `file_write_page` | 496 | 22 |  |
| syscall/edges.rs | `arch_map_user_page` | 518 | 22 |  |
| syscall/edges.rs | `slice_of_kernel_bytes` | 540 | 5 |  |
| syscall/mod.rs | `seam_register_default` | 25 | 266 |  |
| syscall/mod.rs | `new` | 291 | 4 | yes |
| syscall/mod.rs | `set` | 295 | 7 | yes |
| syscall/mod.rs | `clear` | 302 | 7 | yes |
| syscall/mod.rs | `is_set` | 309 | 9 | yes |
| syscall/mod.rs | `zero` | 318 | 11 |  |
| syscall/seccomp.rs | `seccomp_syscall_gate` | 112 | 47 | yes |
| syscall/seccomp.rs | `seccomp_run` | 159 | 67 |  |
| syscall/support.rs | `clamp_rw_count` | 18 | 61 |  |
| syscall/support.rs | `syscall_register_hooks` | 79 | 5 |  |
| syscall/support.rs | `hooks` | 84 | 5 |  |
| syscall/support.rs | `epoll_interest_snapshot` | 89 | 2 |  |
| syscall/support.rs | `timerfd_wheel_audit` | 91 | 2 |  |
| syscall/support.rs | `pid1_epoll_interest_dump` | 93 | 2 |  |
| syscall/support.rs | `kernel_random_bytes` | 95 | 2 |  |
| syscall/support.rs | `oh67_fd_ready` | 97 | 2 |  |
| syscall/support.rs | `jiffies_covering_deadline` | 99 | 2 |  |
| syscall/support.rs | `deadline_ns_to_hres_ticks` | 101 | 2 |  |
| syscall/support.rs | `timerfd_expire_notify` | 103 | 2 | yes |
| syscall/support.rs | `param_stall_dump` | 105 | 2 | yes |
| syscall/support.rs | `is_eventfd_ops` | 107 | 2 |  |
| syscall/support.rs | `syscall_eio_line` | 109 | 2 |  |
| syscall/support.rs | `epoll_dump_for_file` | 111 | 16 |  |
| syscall/support.rs | `new` | 127 | 16 | yes |
