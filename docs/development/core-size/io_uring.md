# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# io_uring — hot 0 lines (stay) / cold 1020 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| io_uring/mod.rs | `try_ref` | 195 | 19 |  |
| io_uring/mod.rs | `unref` | 213 | 12 |  |
| io_uring/mod.rs | `drop` | 224 | 11 |  |
| io_uring/mod.rs | `drop` | 234 | 12 |  |
| io_uring/mod.rs | `sq_ring_size` | 245 | 7 |  |
| io_uring/mod.rs | `cq_ring_size` | 251 | 7 |  |
| io_uring/mod.rs | `sqes_size` | 257 | 5 |  |
| io_uring/mod.rs | `page_align` | 261 | 5 |  |
| io_uring/mod.rs | `round_up_pow2` | 265 | 14 |  |
| io_uring/mod.rs | `alloc_ring_region` | 278 | 27 |  |
| io_uring/mod.rs | `free_ring_region` | 304 | 27 |  |
| io_uring/mod.rs | `io_uring_create` | 330 | 125 |  |
| io_uring/mod.rs | `pin_ring` | 454 | 13 |  |
| io_uring/mod.rs | `io_uring_close` | 466 | 15 |  |
| io_uring/mod.rs | `io_uring_poll` | 480 | 21 |  |
| io_uring/mod.rs | `io_uring_mmap_handler` | 500 | 114 |  |
| io_uring/mod.rs | `submit_sqes` | 613 | 55 |  |
| io_uring/mod.rs | `io_uring_dispatch_op` | 667 | 14 |  |
| io_uring/mod.rs | `io_uring_op_read` | 680 | 37 |  |
| io_uring/mod.rs | `do_read` | 716 | 45 |  |
| io_uring/mod.rs | `io_uring_op_write` | 760 | 36 |  |
| io_uring/mod.rs | `do_write` | 795 | 36 |  |
| io_uring/mod.rs | `io_uring_op_fsync` | 830 | 10 |  |
| io_uring/mod.rs | `io_uring_op_close` | 839 | 22 |  |
| io_uring/mod.rs | `io_uring_op_fadvise` | 860 | 18 |  |
| io_uring/mod.rs | `sq_hdr` | 877 | 4 |  |
| io_uring/mod.rs | `cq_hdr` | 880 | 5 |  |
| io_uring/mod.rs | `sqe_at` | 884 | 14 |  |
| io_uring/mod.rs | `cqe_write` | 897 | 15 |  |
| io_uring/mod.rs | `io_uring_post_cqe` | 911 | 33 |  |
| io_uring/mod.rs | `signal_eventfd_file` | 943 | 18 |  |
| io_uring/mod.rs | `wait_for_cqes` | 960 | 26 |  |
| io_uring/mod.rs | `sys_io_uring_setup` | 985 | 60 |  |
| io_uring/mod.rs | `sys_io_uring_enter` | 1044 | 73 |  |
| io_uring/mod.rs | `sys_io_uring_register` | 1116 | 65 |  |
