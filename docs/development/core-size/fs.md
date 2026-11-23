# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# fs — hot 24134 lines (stay) / cold 21726 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| fs/bio.rs | `new` | 37 | 4 | yes |
| fs/bio.rs | `set` | 41 | 4 | yes |
| fs/bio.rs | `clear` | 45 | 4 | yes |
| fs/bio.rs | `test` | 49 | 4 | yes |
| fs/bio.rs | `is_uptodate` | 53 | 4 | yes |
| fs/bio.rs | `is_dirty` | 57 | 4 | yes |
| fs/bio.rs | `is_locked` | 61 | 4 | yes |
| fs/bio.rs | `is_mapped` | 65 | 33 | yes |
| fs/bio.rs | `new` | 98 | 19 | yes |
| fs/bio.rs | `new_uninit` | 117 | 18 | yes |
| fs/bio.rs | `set_device` | 135 | 5 | yes |
| fs/bio.rs | `get_state` | 140 | 6 | yes |
| fs/bio.rs | `set_state_bit` | 146 | 17 | yes |
| fs/bio.rs | `clear_state_bit` | 163 | 6 | yes |
| fs/bio.rs | `is_dirty` | 169 | 6 | yes |
| fs/bio.rs | `get` | 175 | 5 | yes |
| fs/bio.rs | `put` | 180 | 5 | yes |
| fs/bio.rs | `count` | 185 | 5 | yes |
| fs/bio.rs | `read` | 190 | 11 | yes |
| fs/bio.rs | `write` | 201 | 12 | yes |
| fs/bio.rs | `sync` | 213 | 44 | yes |
| fs/bio.rs | `new` | 257 | 19 | yes |
| fs/bio.rs | `bh` | 276 | 16 | yes |
| fs/bio.rs | `bh_mut` | 292 | 18 | yes |
| fs/bio.rs | `entry_ref` | 310 | 7 | yes |
| fs/bio.rs | `drop` | 317 | 72 | yes |
| fs/bio.rs | `cache_block_size` | 389 | 4 | yes |
| fs/bio.rs | `new` | 393 | 22 | yes |
| fs/bio.rs | `lru_lock_under_bucket` | 415 | 10 | yes |
| fs/bio.rs | `lru_lock_under_bucket_irqsave` | 425 | 8 | yes |
| fs/bio.rs | `hash_index` | 433 | 10 | yes |
| fs/bio.rs | `move_to_lru_head` | 443 | 45 | yes |
| fs/bio.rs | `push_lru_head` | 488 | 25 | yes |
| fs/bio.rs | `remove_from_lru` | 513 | 25 | yes |
| fs/bio.rs | `evict_one` | 538 | 181 | yes |
| fs/bio.rs | `drop_block` | 719 | 61 | yes |
| fs/bio.rs | `get` | 780 | 239 | yes |
| fs/bio.rs | `get_zero` | 1019 | 12 | yes |
| fs/bio.rs | `get_overwrite` | 1031 | 4 | yes |
| fs/bio.rs | `get_zero_impl` | 1035 | 123 | yes |
| fs/bio.rs | `put` | 1158 | 12 | yes |
| fs/bio.rs | `sync_all` | 1170 | 63 | yes |
| fs/bio.rs | `sync_all_excluding` | 1233 | 46 | yes |
| fs/bio.rs | `invalidate` | 1279 | 39 | yes |
| fs/bio.rs | `invalidate_device` | 1318 | 54 | yes |
| fs/bio.rs | `drop` | 1372 | 16 | yes |
| fs/bio.rs | `get_block_cache` | 1388 | 50 | yes |
| fs/bio.rs | `bh_pinned` | 1438 | 7 | yes |
| fs/bio.rs | `bh_data` | 1445 | 5 | yes |
| fs/bio.rs | `bh_data_mut` | 1450 | 10 |  |
| fs/bio.rs | `bh_blocknr` | 1460 | 5 | yes |
| fs/bio.rs | `bh_set_state_bit` | 1465 | 5 | yes |
| fs/bio.rs | `bh_clear_state_bit` | 1470 | 5 | yes |
| fs/bio.rs | `bh_is_dirty` | 1475 | 4 | yes |
| fs/bio.rs | `bread` | 1479 | 8 | yes |
| fs/bio.rs | `getblk_zero` | 1487 | 8 | yes |
| fs/bio.rs | `getblk_overwrite` | 1495 | 7 | yes |
| fs/bio.rs | `drop_freed_block` | 1502 | 11 | yes |
| fs/bio.rs | `drop_freed_block_forgotten` | 1513 | 22 | yes |
| fs/bio.rs | `invalidate_device` | 1535 | 26 | yes |
| fs/bio.rs | `wait_buffer_io_done` | 1561 | 38 | yes |
| fs/bio.rs | `bread_async` | 1599 | 261 | yes |
| fs/bio.rs | `bread_wait` | 1860 | 37 | yes |
| fs/bio.rs | `brelse` | 1897 | 15 | yes |
| fs/bio.rs | `block_cached` | 1912 | 49 | yes |
| fs/bio.rs | `block_dirty_cached` | 1961 | 45 | yes |
| fs/bio.rs | `block_absent_durable` | 2006 | 41 | yes |
| fs/bio.rs | `bread_async_multi` | 2047 | 42 | yes |
| fs/bio.rs | `bfree_multi` | 2089 | 136 | yes |
| fs/bio.rs | `as_ptr` | 2225 | 8 | yes |
| fs/bio.rs | `as_slice` | 2233 | 19 | yes |
| fs/bio.rs | `len` | 2252 | 9 | yes |
| fs/bio.rs | `new` | 2261 | 12 | yes |
| fs/bio.rs | `in_flight` | 2273 | 8 | yes |
| fs/bio.rs | `status` | 2281 | 11 | yes |
| fs/bio.rs | `mode_probe` | 2292 | 20 | yes |
| fs/bio.rs | `submit` | 2312 | 56 | yes |
| fs/bio.rs | `submit_ref` | 2368 | 50 | yes |
| fs/bio.rs | `flush` | 2418 | 13 | yes |
| fs/bio.rs | `flush_immediate` | 2431 | 28 | yes |
| fs/bio.rs | `wait_status_apply` | 2459 | 65 | yes |
| fs/bio.rs | `flush_deferred` | 2524 | 187 | yes |
| fs/bio.rs | `retire_window` | 2711 | 15 | yes |
| fs/bio.rs | `default` | 2726 | 10 | yes |
| fs/bio.rs | `drop` | 2736 | 19 | yes |
| fs/bio.rs | `sync_dirty_blocks_batched` | 2755 | 5 | yes |
| fs/bio.rs | `drain` | 2760 | 99 | yes |
| fs/bio.rs | `sync_dirty_buffer` | 2859 | 34 | yes |
| fs/bio.rs | `sync_buffers` | 2893 | 12 | yes |
| fs/bio.rs | `sync_device_buffers_excluding` | 2905 | 10 | yes |
| fs/bio.rs | `init` | 2915 | 119 | yes |
| fs/bio.rs | `new` | 3034 | 6 | yes |
| fs/bio.rs | `index_record` | 3040 | 8 | yes |
| fs/bio.rs | `push` | 3048 | 5 | yes |
| fs/bio.rs | `post_capture` | 3053 | 12 | yes |
| fs/bio.rs | `take_list` | 3065 | 5 | yes |
| fs/bio.rs | `set_list` | 3070 | 7 | yes |
| fs/bio.rs | `is_empty` | 3077 | 3 | yes |
| fs/bio.rs | `len` | 3080 | 3 | yes |
| fs/bio.rs | `iter` | 3083 | 5 | yes |
| fs/bio.rs | `constrained` | 3088 | 31 | yes |
| fs/bio.rs | `publication_batch_drain` | 3119 | 19 | yes |
| fs/bio.rs | `publication_cookie` | 3138 | 7 | yes |
| fs/bio.rs | `dev_key` | 3145 | 6 | yes |
| fs/bio.rs | `dev_key_of` | 3151 | 7 | yes |
| fs/bio.rs | `publication_defer` | 3158 | 41 | yes |
| fs/bio.rs | `publication_defer_entry_only` | 3199 | 38 | yes |
| fs/bio.rs | `publication_capture_note_dirty` | 3237 | 58 | yes |
| fs/bio.rs | `publication_constrained` | 3295 | 72 | yes |
| fs/bio.rs | `new` | 3367 | 4 | yes |
| fs/bio.rs | `note_queue_entries` | 3371 | 13 | yes |
| fs/bio.rs | `push_pre` | 3384 | 14 | yes |
| fs/bio.rs | `push_entry` | 3398 | 6 | yes |
| fs/bio.rs | `push_post` | 3404 | 39 | yes |
| fs/bio.rs | `plan_add_constraints` | 3443 | 61 | yes |
| fs/bio.rs | `scheduled` | 3504 | 4 | yes |
| fs/bio.rs | `scheduled_pre_of` | 3508 | 10 | yes |
| fs/bio.rs | `flush_publications_scoped` | 3518 | 100 | yes |
| fs/bio.rs | `flush_publication_constraints` | 3618 | 71 | yes |
| fs/bio.rs | `flush_publications` | 3689 | 9 | yes |
| fs/bio.rs | `flush_publications_bounded` | 3698 | 52 | yes |
| fs/bio.rs | `sync_block_ordered` | 3750 | 18 | yes |
| fs/bio.rs | `peek_buffer` | 3768 | 29 | yes |
| fs/bio.rs | `with_block_mut` | 3797 | 29 |  |
| fs/bio.rs | `with_block_deferred_mut` | 3826 | 23 |  |
| fs/bio.rs | `with_block` | 3849 | 23 |  |
| fs/bio.rs | `with_block_ro` | 3872 | 11 |  |
| fs/buffer.rs | `new` | 38 | 14 | yes |
| fs/buffer.rs | `from_bytes` | 52 | 16 | yes |
| fs/buffer.rs | `read` | 68 | 11 | yes |
| fs/buffer.rs | `write` | 79 | 11 | yes |
| fs/buffer.rs | `get` | 90 | 5 | yes |
| fs/buffer.rs | `put` | 95 | 15 | yes |
| fs/buffer.rs | `new` | 110 | 8 | yes |
| fs/buffer.rs | `get_size` | 118 | 5 | yes |
| fs/buffer.rs | `set_size` | 123 | 7 | yes |
| fs/buffer.rs | `read` | 130 | 51 | yes |
| fs/buffer.rs | `write` | 181 | 44 | yes |
| fs/buffer.rs | `load_from_bytes` | 225 | 28 | yes |
| fs/buffer.rs | `truncate` | 253 | 13 | yes |
| fs/buffer.rs | `default` | 266 | 28 | yes |
| fs/buffer.rs | `new` | 294 | 25 | yes |
| fs/buffer.rs | `base_ptr` | 319 | 14 | yes |
| fs/buffer.rs | `drop` | 333 | 19 | yes |
| fs/buffer.rs | `new_chunk` | 352 | 10 | yes |
| fs/buffer.rs | `new` | 362 | 8 | yes |
| fs/buffer.rs | `from_bytes` | 370 | 7 | yes |
| fs/buffer.rs | `read` | 377 | 27 | yes |
| fs/buffer.rs | `write` | 404 | 42 | yes |
| fs/buffer.rs | `resize` | 446 | 35 | yes |
| fs/buffer.rs | `len` | 481 | 5 | yes |
| fs/buffer.rs | `is_empty` | 486 | 6 | yes |
| fs/buffer.rs | `default` | 492 | 4 | yes |
| fs/cgroup.rs | `control_file_by_name` | 122 | 26 |  |
| fs/cgroup.rs | `ino_of_dir` | 148 | 4 |  |
| fs/cgroup.rs | `ino_of_file` | 152 | 8 |  |
| fs/cgroup.rs | `generate_procs` | 160 | 8 |  |
| fs/cgroup.rs | `generate_controllers` | 168 | 11 |  |
| fs/cgroup.rs | `generate_subtree_control` | 179 | 7 |  |
| fs/cgroup.rs | `generate_events` | 186 | 4 |  |
| fs/cgroup.rs | `generate_type` | 190 | 4 |  |
| fs/cgroup.rs | `generate_cpu_max` | 194 | 14 |  |
| fs/cgroup.rs | `generate_cpu_stat` | 208 | 11 |  |
| fs/cgroup.rs | `generate_memory_max` | 219 | 13 |  |
| fs/cgroup.rs | `generate_memory_current` | 232 | 7 |  |
| fs/cgroup.rs | `generate_memory_stat` | 239 | 7 |  |
| fs/char_dev.rs | `new` | 34 | 5 | yes |
| fs/char_dev.rs | `read` | 39 | 8 | yes |
| fs/char_dev.rs | `write` | 47 | 8 | yes |
| fs/char_dev.rs | `uart_read` | 55 | 42 |  |
| fs/char_dev.rs | `uart_write_kernel` | 97 | 15 |  |
| fs/char_dev.rs | `uart_write` | 112 | 24 |  |
| fs/char_dev.rs | `uart_file_poll` | 136 | 32 |  |
| fs/char_dev.rs | `char_dev_file_stat` | 168 | 25 |  |
| fs/char_dev.rs | `uart_file_read` | 193 | 17 |  |
| fs/char_dev.rs | `uart_file_write` | 210 | 10 |  |
| fs/char_dev.rs | `char_dev_stat` | 220 | 32 | yes |
| fs/debugfs.rs | `next_ino` | 91 | 5 |  |
| fs/debugfs.rs | `debugfs_root` | 96 | 12 |  |
| fs/debugfs.rs | `create_dir` | 108 | 42 |  |
| fs/debugfs.rs | `create_file` | 150 | 78 |  |
| fs/debugfs.rs | `debugfs_mount_cb` | 228 | 12 |  |
| fs/debugfs.rs | `debugfs_kill_sb` | 240 | 11 |  |
| fs/debugfs.rs | `init_debugfs` | 251 | 33 |  |
| fs/debugfs.rs | `is_mounted` | 284 | 6 |  |
| fs/debugfs.rs | `mount_debugfs` | 290 | 16 |  |
| fs/debugfs.rs | `create_root_inode` | 306 | 31 |  |
| fs/debugfs.rs | `entry_of` | 337 | 5 |  |
| fs/debugfs.rs | `resolve_child` | 342 | 10 |  |
| fs/debugfs.rs | `debugfs_lookup` | 352 | 10 |  |
| fs/debugfs.rs | `debugfs_iget` | 362 | 21 |  |
| fs/debugfs.rs | `debugfs_getattr` | 383 | 33 |  |
| fs/debugfs.rs | `debugfs_setattr` | 416 | 9 |  |
| fs/debugfs.rs | `debugfs_readdir` | 425 | 25 |  |
| fs/debugfs.rs | `debugfs_open` | 450 | 19 |  |
| fs/debugfs.rs | `debugfs_get_file_ops` | 469 | 12 |  |
| fs/debugfs.rs | `debugfs_destroy_inode` | 481 | 43 |  |
| fs/debugfs.rs | `debugfs_file_read` | 524 | 20 |  |
| fs/debugfs.rs | `debugfs_file_write` | 544 | 25 |  |
| fs/debugfs.rs | `debugfs_file_lseek` | 569 | 21 |  |
| fs/debugfs.rs | `debugfs_file_close` | 590 | 23 |  |
| fs/debugfs.rs | `mount_debugfs_instance` | 613 | 14 |  |
| fs/debugfs.rs | `mount_register` | 627 | 3 |  |
| fs/dentry.rs | `new` | 40 | 4 | yes |
| fs/dentry.rs | `is_hashed` | 44 | 4 | yes |
| fs/dentry.rs | `is_unhashed` | 48 | 4 | yes |
| fs/dentry.rs | `bits` | 52 | 39 | yes |
| fs/dentry.rs | `new` | 91 | 104 | yes |
| fs/dentry.rs | `acquire` | 195 | 17 | yes |
| fs/dentry.rs | `drop` | 212 | 12 | yes |
| fs/dentry.rs | `retire_dentry_arc` | 224 | 5 | yes |
| fs/dentry.rs | `retire_inode_arc` | 229 | 23 | yes |
| fs/dentry.rs | `dentry_drain_if_quiescent` | 252 | 70 | yes |
| fs/dentry.rs | `dentry_arc_from_raw` | 322 | 9 | yes |
| fs/dentry.rs | `inode_arc_from_raw` | 331 | 7 | yes |
| fs/dentry.rs | `child_name_hash` | 338 | 83 | yes |
| fs/dentry.rs | `empty` | 421 | 18 | yes |
| fs/dentry.rs | `memo_slot_read` | 439 | 121 | yes |
| fs/dentry.rs | `new` | 560 | 27 | yes |
| fs/dentry.rs | `neg_gen_load` | 587 | 20 | yes |
| fs/dentry.rs | `memo_negative` | 607 | 50 | yes |
| fs/dentry.rs | `set_parent` | 657 | 5 | yes |
| fs/dentry.rs | `set_inode` | 662 | 26 | yes |
| fs/dentry.rs | `clear_inode` | 688 | 16 | yes |
| fs/dentry.rs | `inode_ptr_walk` | 704 | 6 | yes |
| fs/dentry.rs | `get_inode_fast` | 710 | 15 | yes |
| fs/dentry.rs | `get_inode_gen` | 725 | 6 | yes |
| fs/dentry.rs | `set_inode_gen` | 731 | 5 | yes |
| fs/dentry.rs | `get_inode` | 736 | 5 | yes |
| fs/dentry.rs | `get_name` | 741 | 5 | yes |
| fs/dentry.rs | `build_path` | 746 | 22 | yes |
| fs/dentry.rs | `build_path_rooted` | 768 | 90 | yes |
| fs/dentry.rs | `lookup_child` | 858 | 21 | yes |
| fs/dentry.rs | `lookup_child_walk` | 879 | 54 | yes |
| fs/dentry.rs | `lookup_child_fast` | 933 | 18 | yes |
| fs/dentry.rs | `refill_memo_locked` | 951 | 73 | yes |
| fs/dentry.rs | `big_memo_ensure_locked` | 1024 | 49 | yes |
| fs/dentry.rs | `add_child` | 1073 | 45 | yes |
| fs/dentry.rs | `remove_child` | 1118 | 31 | yes |
| fs/dentry.rs | `clear_children` | 1149 | 20 | yes |
| fs/dentry.rs | `rehome_children_into` | 1169 | 27 | yes |
| fs/dentry.rs | `clear_mount` | 1196 | 11 | yes |
| fs/dentry.rs | `take_mount` | 1207 | 7 | yes |
| fs/dentry.rs | `set_mount` | 1214 | 6 | yes |
| fs/dentry.rs | `get_mount` | 1220 | 9 | yes |
| fs/dentry.rs | `has_mount` | 1229 | 6 | yes |
| fs/dentry.rs | `set_mount_flag` | 1235 | 7 | yes |
| fs/dentry.rs | `set_mountpoint_back` | 1242 | 7 | yes |
| fs/dentry.rs | `get_mountpoint_back` | 1249 | 5 | yes |
| fs/dentry.rs | `clear_mountpoint_back` | 1254 | 6 | yes |
| fs/dentry.rs | `mountpoint_dentry` | 1260 | 5 | yes |
| fs/dentry.rs | `set_hashed` | 1265 | 7 | yes |
| fs/dentry.rs | `set_unhashed` | 1272 | 7 | yes |
| fs/dentry.rs | `inc_ref` | 1279 | 5 | yes |
| fs/dentry.rs | `dec_ref` | 1284 | 5 | yes |
| fs/dentry.rs | `get_ref` | 1289 | 5 | yes |
| fs/dentry.rs | `set_negative` | 1294 | 5 | yes |
| fs/dentry.rs | `is_negative` | 1299 | 5 | yes |
| fs/dentry.rs | `clear_negative` | 1304 | 6 | yes |
| fs/dentry.rs | `make_root_dentry` | 1310 | 26 | yes |
| fs/dentry.rs | `new` | 1336 | 8 | yes |
| fs/dentry.rs | `record_hit` | 1344 | 4 | yes |
| fs/dentry.rs | `record_miss` | 1348 | 4 | yes |
| fs/dentry.rs | `record_eviction` | 1352 | 4 | yes |
| fs/dentry.rs | `get_hit_rate` | 1356 | 25 | yes |
| fs/dentry.rs | `clone` | 1381 | 29 | yes |
| fs/dentry.rs | `dcache_init` | 1410 | 27 | yes |
| fs/dentry.rs | `dentry_hash` | 1437 | 18 | yes |
| fs/dentry.rs | `dcache_lookup` | 1455 | 39 | yes |
| fs/dentry.rs | `dcache_add` | 1494 | 47 | yes |
| fs/dentry.rs | `dcache_evict_lru` | 1541 | 37 | yes |
| fs/dentry.rs | `dcache_remove` | 1578 | 27 | yes |
| fs/dentry.rs | `dcache_stats` | 1605 | 11 | yes |
| fs/dentry.rs | `dcache_stats_detailed` | 1616 | 17 | yes |
| fs/dentry.rs | `dcache_flush` | 1633 | 20 | yes |
| fs/dev_t.rs | `new` | 29 | 5 | yes |
| fs/dev_t.rs | `from_u64` | 34 | 8 |  |
| fs/dev_t.rs | `to_u64` | 42 | 9 |  |
| fs/dev_t.rs | `to_user_dev` | 51 | 11 |  |
| fs/dev_t.rs | `from_user_dev` | 62 | 9 |  |
| fs/dev_t.rs | `default` | 71 | 78 | yes |
| fs/devfs/mod.rs | `new_dir` | 126 | 16 |  |
| fs/devfs/mod.rs | `new_file` | 142 | 20 |  |
| fs/devfs/mod.rs | `new_symlink` | 162 | 16 |  |
| fs/devfs/mod.rs | `new_char_device` | 178 | 16 |  |
| fs/devfs/mod.rs | `new_char_device_with_mode` | 194 | 18 |  |
| fs/devfs/mod.rs | `new_block_device` | 212 | 16 |  |
| fs/devfs/mod.rs | `is_dir` | 228 | 5 |  |
| fs/devfs/mod.rs | `is_char_device` | 233 | 5 | yes |
| fs/devfs/mod.rs | `is_block_device` | 238 | 5 | yes |
| fs/devfs/mod.rs | `is_regular_file` | 243 | 5 | yes |
| fs/devfs/mod.rs | `is_symlink` | 248 | 17 | yes |
| fs/devfs/mod.rs | `nulldev_read` | 265 | 4 |  |
| fs/devfs/mod.rs | `nulldev_write` | 269 | 7 |  |
| fs/devfs/mod.rs | `nulldev_poll` | 276 | 22 |  |
| fs/devfs/mod.rs | `condev_read` | 298 | 9 |  |
| fs/devfs/mod.rs | `condev_write` | 307 | 43 |  |
| fs/devfs/mod.rs | `condev_poll` | 350 | 19 |  |
| fs/devfs/mod.rs | `zerodev_read` | 369 | 5 |  |
| fs/devfs/mod.rs | `zerodev_write` | 374 | 5 |  |
| fs/devfs/mod.rs | `zerodev_poll` | 379 | 12 |  |
| fs/devfs/mod.rs | `fulldev_write` | 391 | 15 |  |
| fs/devfs/mod.rs | `rng_fill` | 406 | 17 |  |
| fs/devfs/mod.rs | `randdev_read` | 423 | 5 |  |
| fs/devfs/mod.rs | `randdev_poll` | 428 | 32 |  |
| fs/devfs/mod.rs | `root_disk_present` | 460 | 18 |  |
| fs/devfs/mod.rs | `devtmpfs_populate` | 478 | 229 |  |
| fs/devfs/mod.rs | `mknod_user` | 707 | 15 |  |
| fs/devfs/mod.rs | `evict_dev_dentry` | 722 | 23 |  |
| fs/devfs/mod.rs | `devtmpfs_register_device` | 745 | 32 |  |
| fs/devfs/mod.rs | `devtmpfs_unregister_device` | 777 | 22 |  |
| fs/devfs/mod.rs | `init` | 799 | 148 | yes |
| fs/devfs/mod.rs | `mknod` | 947 | 71 |  |
| fs/devfs/mod.rs | `remove_node` | 1018 | 64 |  |
| fs/devfs/mod.rs | `mkdir` | 1082 | 50 |  |
| fs/devfs/mod.rs | `lookup` | 1132 | 45 | yes |
| fs/devfs/mod.rs | `is_mounted` | 1177 | 5 |  |
| fs/devfs/mod.rs | `get_root_entry` | 1182 | 14 |  |
| fs/devfs/mod.rs | `list_dir` | 1196 | 48 |  |
| fs/devfs/mod.rs | `parse_dev_path` | 1244 | 20 |  |
| fs/devfs/mod.rs | `devfs_lookup` | 1264 | 21 |  |
| fs/devfs/mod.rs | `devfs_iget` | 1285 | 57 |  |
| fs/devfs/mod.rs | `devfs_getattr` | 1342 | 47 |  |
| fs/devfs/mod.rs | `devfs_ino_hash` | 1389 | 26 |  |
| fs/devfs/mod.rs | `devfs_open` | 1415 | 77 |  |
| fs/devfs/mod.rs | `devfs_get_file_ops` | 1492 | 35 |  |
| fs/devfs/mod.rs | `devfs_readdir` | 1527 | 37 |  |
| fs/devfs/mod.rs | `devfs_destroy_inode` | 1564 | 16 |  |
| fs/devfs/mod.rs | `devfs_rel_path` | 1580 | 1 |  |
| fs/devfs/mod.rs | `walk` | 1581 | 28 | yes |
| fs/devfs/mod.rs | `devfs_mkdir` | 1609 | 37 |  |
| fs/devfs/mod.rs | `devfs_create` | 1646 | 34 |  |
| fs/devfs/mod.rs | `devfs_symlink` | 1680 | 36 |  |
| fs/devfs/mod.rs | `devfs_readlink` | 1716 | 21 |  |
| fs/devfs/mod.rs | `devfs_link` | 1737 | 56 |  |
| fs/devfs/mod.rs | `devfs_rename` | 1793 | 97 |  |
| fs/devfs/mod.rs | `new` | 1890 | 7 | yes |
| fs/devfs/mod.rs | `copy_in` | 1897 | 9 |  |
| fs/devfs/mod.rs | `copy_out` | 1906 | 13 | yes |
| fs/devfs/mod.rs | `zero` | 1919 | 6 |  |
| fs/devfs/mod.rs | `shared_grow_locked` | 1925 | 75 |  |
| fs/devfs/mod.rs | `devfs_reg_current_size` | 2000 | 18 |  |
| fs/devfs/mod.rs | `shared_fault_file_page` | 2018 | 51 |  |
| fs/devfs/mod.rs | `shared_entry_of` | 2069 | 20 |  |
| fs/devfs/mod.rs | `shared_read_off` | 2089 | 25 |  |
| fs/devfs/mod.rs | `shared_write_off` | 2114 | 34 |  |
| fs/devfs/mod.rs | `devfs_file_read` | 2148 | 31 |  |
| fs/devfs/mod.rs | `devfs_file_write` | 2179 | 36 |  |
| fs/devfs/mod.rs | `devfs_file_lseek` | 2215 | 33 |  |
| fs/devfs/mod.rs | `devfs_file_read_at` | 2248 | 42 |  |
| fs/devfs/mod.rs | `devfs_unlink` | 2290 | 32 |  |
| fs/devfs/mod.rs | `devfs_rmdir` | 2322 | 50 |  |
| fs/devfs/mod.rs | `devfs_setattr` | 2372 | 107 |  |
| fs/devfs/mod.rs | `create_root_inode` | 2479 | 15 |  |
| fs/devfs/mod.rs | `mount_devtmpfs` | 2494 | 12 |  |
| fs/devfs/mod.rs | `mount_register` | 2506 | 3 |  |
| fs/devfs/registry.rs | `new` | 27 | 18 | yes |
| fs/devfs/registry.rs | `register_char_device` | 45 | 13 |  |
| fs/devfs/registry.rs | `unregister_char_device` | 58 | 12 |  |
| fs/devfs/registry.rs | `get_char_device_ops` | 70 | 6 |  |
| fs/devfs/registry.rs | `is_device_registered` | 76 | 6 |  |
| fs/devfs/registry.rs | `device_count` | 82 | 4 |  |
| fs/elf.rs | `from_bytes` | 348 | 47 | yes |
| fs/elf.rs | `is_executable` | 395 | 6 | yes |
| fs/elf.rs | `is_dynamic` | 401 | 5 |  |
| fs/elf.rs | `check_machine` | 406 | 16 |  |
| fs/elf.rs | `get_program_headers` | 422 | 11 |  |
| fs/elf.rs | `get_program_header` | 433 | 20 | yes |
| fs/elf.rs | `is_load` | 453 | 5 |  |
| fs/elf.rs | `is_readable` | 458 | 5 | yes |
| fs/elf.rs | `is_writable` | 463 | 5 | yes |
| fs/elf.rs | `is_executable` | 468 | 24 | yes |
| fs/elf.rs | `validate` | 492 | 25 |  |
| fs/elf.rs | `get_entry` | 517 | 7 |  |
| fs/elf.rs | `get_program_headers` | 524 | 15 |  |
| fs/elf.rs | `load` | 539 | 85 | yes |
| fs/elf.rs | `load_segment` | 624 | 34 |  |
| fs/elf.rs | `get_interpreter` | 658 | 50 |  |
| fs/event_notify.rs | `list_first` | 77 | 7 |  |
| fs/event_notify.rs | `list_next` | 84 | 6 | yes |
| fs/event_notify.rs | `list_link_tail` | 90 | 8 |  |
| fs/event_notify.rs | `list_unlink` | 98 | 7 |  |
| fs/event_notify.rs | `boxed_mut` | 105 | 6 | yes |
| fs/event_notify.rs | `boxed_drop` | 111 | 7 |  |
| fs/event_notify.rs | `list_entry_mut` | 118 | 7 |  |
| fs/event_notify.rs | `list_entry_pub` | 125 | 5 |  |
| fs/event_notify.rs | `list_entry` | 130 | 12 |  |
| fs/event_notify.rs | `notify_pin_acquire` | 142 | 2 |  |
| fs/event_notify.rs | `notify_pin_release` | 144 | 2 |  |
| fs/event_notify.rs | `rearm_edge` | 146 | 2 |  |
| fs/event_notify.rs | `wake_waiters` | 148 | 4 |  |
| fs/event_notify.rs | `boxed_ref` | 152 | 51 |  |
| fs/event_notify.rs | `locked_head` | 203 | 4 |  |
| fs/event_notify.rs | `init_once` | 207 | 36 |  |
| fs/event_notify.rs | `epoll_shard_index` | 243 | 7 |  |
| fs/event_notify.rs | `epoll_wake_shard` | 250 | 21 |  |
| fs/event_notify.rs | `epoll_notify_file` | 271 | 85 |  |
| fs/event_notify.rs | `no_wake` | 356 | 75 |  |
| fs/event_notify.rs | `epoll_wake_register` | 431 | 53 |  |
| fs/event_notify.rs | `epoll_wake_unregister_all` | 484 | 52 |  |
| fs/event_notify.rs | `epoll_wake_unregister` | 536 | 53 |  |
| fs/event_notify.rs | `signalfd_notify_task` | 589 | 27 |  |
| fs/event_notify.rs | `signalfd_register` | 616 | 10 |  |
| fs/event_notify.rs | `signalfd_unregister` | 626 | 10 |  |
| fs/ext4_edges.rs | `typed_ref` | 23 | 10 |  |
| fs/ext4_edges.rs | `typed_ref_mut` | 33 | 13 |  |
| fs/ext4_edges.rs | `typed_slice` | 46 | 20 |  |
| fs/ext4_edges.rs | `typed_slice_mut` | 66 | 23 |  |
| fs/ext4_edges.rs | `u32_array_ref` | 89 | 11 |  |
| fs/ext4_edges.rs | `u32_array_ref_mut` | 100 | 17 |  |
| fs/ext4_edges.rs | `zeroed_on_disk` | 117 | 5 |  |
| fs/ext4_hooks.rs | `ext4_register_hooks` | 164 | 5 |  |
| fs/ext4_hooks.rs | `hooks` | 169 | 6 |  |
| fs/ext4_hooks.rs | `init` | 175 | 3 | yes |
| fs/ext4_hooks.rs | `mount_root` | 178 | 3 |  |
| fs/ext4_hooks.rs | `root_inode` | 181 | 3 |  |
| fs/ext4_hooks.rs | `mounted` | 184 | 3 |  |
| fs/ext4_hooks.rs | `unmount` | 187 | 3 |  |
| fs/ext4_hooks.rs | `mount_instance` | 190 | 3 |  |
| fs/ext4_hooks.rs | `loop_invalidate` | 193 | 3 |  |
| fs/ext4_hooks.rs | `boot_read_file` | 196 | 3 |  |
| fs/ext4_hooks.rs | `fsctx_mount_instance` | 199 | 3 |  |
| fs/ext4_hooks.rs | `mount_rows_register` | 202 | 3 |  |
| fs/ext4_hooks.rs | `sync_flushed_data` | 205 | 3 | yes |
| fs/ext4_hooks.rs | `sync_inode_tables` | 208 | 3 | yes |
| fs/ext4_hooks.rs | `shutdown_sync` | 211 | 3 |  |
| fs/ext4_hooks.rs | `remount_ro_latch` | 214 | 3 | yes |
| fs/ext4_hooks.rs | `fallocate` | 217 | 3 | yes |
| fs/ext4_hooks.rs | `default_inode_ops` | 220 | 3 |  |
| fs/ext4_hooks.rs | `is_ext4_inode_ops` | 223 | 3 | yes |
| fs/ext4_hooks.rs | `is_ext4_lookup` | 226 | 3 | yes |
| fs/ext4_hooks.rs | `is_ext4_file` | 229 | 3 |  |
| fs/ext4_hooks.rs | `writeback_flush` | 232 | 3 | yes |
| fs/ext4_hooks.rs | `kmsg_resolve` | 235 | 3 |  |
| fs/ext4_hooks.rs | `kmsg_write_at` | 238 | 3 |  |
| fs/ext4_hooks.rs | `journal_dirty` | 241 | 3 | yes |
| fs/ext4_hooks.rs | `journal_forget` | 244 | 3 | yes |
| fs/ext4_hooks.rs | `exit_journal_unwind` | 247 | 3 |  |
| fs/ext4_hooks.rs | `exthunt_phys_watched` | 250 | 3 | yes |
| fs/ext4_hooks.rs | `exthunt_evict_unhash` | 253 | 3 | yes |
| fs/ext4_hooks.rs | `exthunt_evict_done` | 256 | 3 | yes |
| fs/ext4_hooks.rs | `exthunt_fab` | 259 | 3 | yes |
| fs/ext4_hooks.rs | `exthunt_bc_blind` | 262 | 3 | yes |
| fs/ext4_hooks.rs | `exthunt_rxbhit` | 265 | 3 | yes |
| fs/ext4_hooks.rs | `demand_read_stats` | 268 | 3 |  |
| fs/fdreap.rs | `retire_complete` | 132 | 37 | yes |
| fs/fdreap.rs | `drain_with_grace` | 169 | 31 |  |
| fs/fdreap.rs | `init` | 200 | 60 | yes |
| fs/file.rs | `new` | 71 | 4 | yes |
| fs/file.rs | `is_readonly` | 75 | 4 | yes |
| fs/file.rs | `is_writeonly` | 79 | 4 | yes |
| fs/file.rs | `is_rdwr` | 83 | 4 | yes |
| fs/file.rs | `bits` | 87 | 5 | yes |
| fs/file.rs | `set_bits` | 92 | 5 | yes |
| fs/file.rs | `add_flags` | 97 | 59 | yes |
| fs/file.rs | `builder` | 156 | 22 | yes |
| fs/file.rs | `read` | 178 | 5 | yes |
| fs/file.rs | `write` | 183 | 5 | yes |
| fs/file.rs | `lseek` | 188 | 5 | yes |
| fs/file.rs | `close` | 193 | 7 | yes |
| fs/file.rs | `flush` | 200 | 5 | yes |
| fs/file.rs | `poll` | 205 | 5 | yes |
| fs/file.rs | `read_at` | 210 | 6 | yes |
| fs/file.rs | `write_at` | 216 | 6 | yes |
| fs/file.rs | `fault` | 222 | 5 | yes |
| fs/file.rs | `build` | 227 | 52 | yes |
| fs/file.rs | `next_file_id` | 279 | 25 | yes |
| fs/file.rs | `file_arc_try_clone` | 304 | 31 | yes |
| fs/file.rs | `verify_arc_inner_layout` | 335 | 122 | yes |
| fs/file.rs | `none` | 457 | 25 | yes |
| fs/file.rs | `new` | 482 | 20 | yes |
| fs/file.rs | `flags` | 502 | 5 | yes |
| fs/file.rs | `set_flags` | 507 | 5 | yes |
| fs/file.rs | `flags_bits` | 512 | 12 | yes |
| fs/file.rs | `set_inode` | 524 | 11 | yes |
| fs/file.rs | `set_dentry` | 535 | 10 | yes |
| fs/file.rs | `get_dentry` | 545 | 6 | yes |
| fs/file.rs | `set_ops` | 551 | 5 | yes |
| fs/file.rs | `get_ops` | 556 | 11 | yes |
| fs/file.rs | `get_inode` | 567 | 7 | yes |
| fs/file.rs | `set_private_data` | 574 | 12 | yes |
| fs/file.rs | `private_data_ptr` | 586 | 8 | yes |
| fs/file.rs | `get_private_data` | 594 | 6 | yes |
| fs/file.rs | `clear_private_data` | 600 | 9 | yes |
| fs/file.rs | `take_private_data` | 609 | 7 | yes |
| fs/file.rs | `get_cloexec` | 616 | 5 | yes |
| fs/file.rs | `set_cloexec` | 621 | 22 | yes |
| fs/file.rs | `missing_op_errno` | 643 | 25 | yes |
| fs/file.rs | `poll_dispatch` | 668 | 19 | yes |
| fs/file.rs | `read` | 687 | 28 | yes |
| fs/file.rs | `write` | 715 | 27 | yes |
| fs/file.rs | `lseek` | 742 | 22 | yes |
| fs/file.rs | `read_at` | 764 | 74 | yes |
| fs/file.rs | `positional_capable` | 838 | 19 | yes |
| fs/file.rs | `write_at` | 857 | 90 | yes |
| fs/file.rs | `read_slice` | 947 | 7 | yes |
| fs/file.rs | `write_slice` | 954 | 7 | yes |
| fs/file.rs | `read_at_slice` | 961 | 6 | yes |
| fs/file.rs | `write_at_slice` | 967 | 8 | yes |
| fs/file.rs | `lseek_op` | 975 | 7 | yes |
| fs/file.rs | `close` | 982 | 10 | yes |
| fs/file.rs | `get_pos` | 992 | 5 | yes |
| fs/file.rs | `set_pos` | 997 | 8 | yes |
| fs/file.rs | `path` | 1005 | 35 | yes |
| fs/file.rs | `drop` | 1040 | 95 | yes |
| fs/file.rs | `new` | 1135 | 26 | yes |
| fs/file.rs | `fork_from` | 1161 | 35 | yes |
| fs/file.rs | `nofile_ceiling` | 1196 | 8 | yes |
| fs/file.rs | `alloc_fd` | 1204 | 9 | yes |
| fs/file.rs | `install_lowest_fd` | 1213 | 47 | yes |
| fs/file.rs | `alloc_fd_from` | 1260 | 29 | yes |
| fs/file.rs | `install_fd` | 1289 | 54 | yes |
| fs/file.rs | `get_file_borrow` | 1343 | 15 | yes |
| fs/file.rs | `get_file` | 1358 | 37 | yes |
| fs/file.rs | `for_each_file` | 1395 | 33 | yes |
| fs/file.rs | `close_fd` | 1428 | 70 | yes |
| fs/file.rs | `close_fd_report` | 1498 | 37 | yes |
| fs/file.rs | `park_and_drain` | 1535 | 5 | yes |
| fs/file.rs | `dup_fd` | 1540 | 15 | yes |
| fs/file.rs | `dup_fd_strict` | 1555 | 28 | yes |
| fs/file.rs | `dup2_fd` | 1583 | 48 | yes |
| fs/file.rs | `set_fd_cloexec` | 1631 | 10 | yes |
| fs/file.rs | `get_fd_cloexec` | 1641 | 7 | yes |
| fs/file.rs | `close_cloexec_fds` | 1648 | 27 | yes |
| fs/file.rs | `drop` | 1675 | 40 | yes |
| fs/file.rs | `set_cloexec_fd` | 1715 | 15 | yes |
| fs/file.rs | `alloc_staging_buf` | 1730 | 13 | yes |
| fs/file.rs | `gather_append_user` | 1743 | 22 | yes |
| fs/file.rs | `get_file_fd` | 1765 | 15 | yes |
| fs/file.rs | `file` | 1780 | 12 | yes |
| fs/file.rs | `get_file_fd_fast` | 1792 | 23 | yes |
| fs/file.rs | `get_file_fd_safe` | 1815 | 8 | yes |
| fs/file.rs | `get_file_fd_install` | 1823 | 7 | yes |
| fs/file.rs | `close_file_fd` | 1830 | 10 | yes |
| fs/file.rs | `close_file_fd_report` | 1840 | 16 | yes |
| fs/file.rs | `get_stdin` | 1856 | 4 | yes |
| fs/file.rs | `get_stdout` | 1860 | 4 | yes |
| fs/file.rs | `get_stderr` | 1864 | 8 | yes |
| fs/file.rs | `reg_file_read` | 1872 | 29 | yes |
| fs/file.rs | `reg_file_read_pub` | 1901 | 5 | yes |
| fs/file.rs | `reg_file_write_pub` | 1906 | 5 | yes |
| fs/file.rs | `reg_file_lseek_pub` | 1911 | 5 | yes |
| fs/file.rs | `reg_file_close_pub` | 1916 | 4 | yes |
| fs/file.rs | `reg_file_write` | 1920 | 67 | yes |
| fs/file.rs | `reg_file_lseek` | 1987 | 32 | yes |
| fs/file.rs | `reg_file_close` | 2019 | 21 | yes |
| fs/file.rs | `dir_read_eisdir` | 2040 | 4 | yes |
| fs/file.rs | `dir_close` | 2044 | 9 | yes |
| fs/file.rs | `dir_file_lseek` | 2053 | 23 | yes |
| fs/file_hooks.rs | `pipe_stat` | 11 | 5 | yes |
| fs/file_hooks.rs | `pipe_get_sz` | 16 | 5 | yes |
| fs/file_hooks.rs | `pipe_set_sz` | 21 | 8 | yes |
| fs/file_hooks.rs | `oasync_register` | 29 | 3 | yes |
| fs/file_hooks.rs | `oasync_unregister` | 32 | 5 | yes |
| fs/file_hooks.rs | `char_dev_stat` | 37 | 7 | yes |
| fs/file_hooks.rs | `fifo_forget` | 44 | 5 | yes |
| fs/file_hooks.rs | `fifo_open` | 49 | 9 | yes |
| fs/file_hooks.rs | `memfd_add_seals` | 58 | 3 | yes |
| fs/file_hooks.rs | `memfd_get_seals` | 61 | 3 | yes |
| fs/fs_hooks.rs | `fs_register_hooks` | 94 | 5 |  |
| fs/fs_hooks.rs | `hooks` | 99 | 4 |  |
| fs/fs_hooks.rs | `notify` | 103 | 3 | yes |
| fs/fs_hooks.rs | `notify_file` | 106 | 3 | yes |
| fs/fs_hooks.rs | `notify_file_close` | 109 | 3 | yes |
| fs/fs_hooks.rs | `alloc_cookie` | 112 | 5 | yes |
| fs/fs_hooks.rs | `is_pipe_ops` | 117 | 1 |  |
| fs/fs_hooks.rs | `is_fifo_ops` | 118 | 1 |  |
| fs/fs_hooks.rs | `is_inotify_ops` | 119 | 1 |  |
| fs/fs_hooks.rs | `is_memfd_ops` | 120 | 1 |  |
| fs/fs_hooks.rs | `pipe_diag_snapshot` | 121 | 1 |  |
| fs/fs_hooks.rs | `memfd_name_by_ino` | 122 | 1 |  |
| fs/fs_struct.rs | `new` | 68 | 13 | yes |
| fs/fs_struct.rs | `with_cwd` | 81 | 15 |  |
| fs/fs_struct.rs | `get_cwd` | 96 | 16 | yes |
| fs/fs_struct.rs | `set_cwd` | 112 | 8 |  |
| fs/fs_struct.rs | `set_cwd_with_dentry` | 120 | 7 |  |
| fs/fs_struct.rs | `cwd_dentry` | 127 | 5 | yes |
| fs/fs_struct.rs | `cwd_slice` | 132 | 9 |  |
| fs/fs_struct.rs | `get_root` | 141 | 15 | yes |
| fs/fs_struct.rs | `root_is_slash` | 156 | 10 |  |
| fs/fs_struct.rs | `set_root` | 166 | 9 |  |
| fs/fs_struct.rs | `set_root_with_dentry` | 175 | 6 | yes |
| fs/fs_struct.rs | `root_dentry` | 181 | 6 | yes |
| fs/fs_struct.rs | `set_pivot_put_old` | 187 | 5 |  |
| fs/fs_struct.rs | `pivot_put_old` | 192 | 5 |  |
| fs/fs_struct.rs | `clear_pivot_put_old` | 197 | 6 |  |
| fs/fs_struct.rs | `root_is_pivot` | 203 | 5 |  |
| fs/fs_struct.rs | `set_root_is_pivot` | 208 | 9 |  |
| fs/fs_struct.rs | `copy_fs_from` | 217 | 16 |  |
| fs/fs_struct.rs | `get_umask` | 233 | 5 | yes |
| fs/fs_struct.rs | `set_umask` | 238 | 5 |  |
| fs/fs_struct.rs | `apply_umask` | 243 | 6 |  |
| fs/fs_struct.rs | `default` | 249 | 7 | yes |
| fs/fsctx.rs | `fs_type_known` | 111 | 31 |  |
| fs/fsctx.rs | `param_key_known` | 142 | 79 |  |
| fs/fsctx.rs | `new` | 221 | 15 | yes |
| fs/fsctx.rs | `new_reconfigure` | 236 | 17 |  |
| fs/fsctx.rs | `data_string` | 253 | 39 |  |
| fs/fsctx.rs | `create_fs_root` | 292 | 95 |  |
| fs/fsctx.rs | `fsctx_close` | 387 | 15 |  |
| fs/fsctx.rs | `fsctx_of` | 402 | 12 |  |
| fs/fsctx.rs | `fsctx_of_mut` | 414 | 17 |  |
| fs/fsctx.rs | `mountfd_close` | 431 | 15 |  |
| fs/fsctx.rs | `mountfd_of` | 446 | 17 |  |
| fs/fsctx.rs | `treepath_dentry` | 463 | 11 |  |
| fs/fsctx.rs | `make_detached_mount` | 474 | 34 |  |
| fs/fsctx.rs | `install_fd_with` | 508 | 24 |  |
| fs/fsctx.rs | `mount_attr_apply` | 532 | 44 |  |
| fs/fsctx.rs | `validate_mount_attr` | 576 | 27 |  |
| fs/fsctx.rs | `validate_mount_attr_pair` | 603 | 30 |  |
| fs/fsctx.rs | `attr_flags_to_mnt` | 633 | 3 |  |
| fs/inode.rs | `new` | 128 | 4 | yes |
| fs/inode.rs | `is_regular_file` | 132 | 4 | yes |
| fs/inode.rs | `is_directory` | 136 | 4 | yes |
| fs/inode.rs | `is_char_device` | 140 | 4 | yes |
| fs/inode.rs | `is_block_device` | 144 | 4 | yes |
| fs/inode.rs | `is_fifo` | 148 | 4 | yes |
| fs/inode.rs | `is_symlink` | 152 | 4 | yes |
| fs/inode.rs | `is_socket` | 156 | 4 |  |
| fs/inode.rs | `bits` | 160 | 163 | yes |
| fs/inode.rs | `call_lookup` | 323 | 13 | yes |
| fs/inode.rs | `call_create` | 336 | 14 | yes |
| fs/inode.rs | `call_link` | 350 | 8 | yes |
| fs/inode.rs | `call_unlink` | 358 | 8 | yes |
| fs/inode.rs | `call_symlink` | 366 | 14 | yes |
| fs/inode.rs | `call_mkdir` | 380 | 14 | yes |
| fs/inode.rs | `call_rmdir` | 394 | 8 | yes |
| fs/inode.rs | `call_mknod` | 402 | 15 | yes |
| fs/inode.rs | `call_rename` | 417 | 15 |  |
| fs/inode.rs | `call_readlink` | 432 | 8 |  |
| fs/inode.rs | `call_get_file_ops` | 440 | 13 | yes |
| fs/inode.rs | `call_readdir` | 453 | 11 | yes |
| fs/inode.rs | `call_readdir_fill` | 464 | 13 | yes |
| fs/inode.rs | `call_open` | 477 | 8 | yes |
| fs/inode.rs | `call_permission` | 485 | 8 |  |
| fs/inode.rs | `call_getattr` | 493 | 8 | yes |
| fs/inode.rs | `call_setattr` | 501 | 15 |  |
| fs/inode.rs | `call_iget` | 516 | 16 | yes |
| fs/inode.rs | `call_destroy_inode` | 532 | 72 |  |
| fs/inode.rs | `stat_dev_for_fs` | 604 | 35 |  |
| fs/inode.rs | `stat_dev_user` | 639 | 104 | yes |
| fs/inode.rs | `drop` | 743 | 23 | yes |
| fs/inode.rs | `dir_mut_gen_bump` | 766 | 6 | yes |
| fs/inode.rs | `dir_mut_gen_load` | 772 | 14 | yes |
| fs/inode.rs | `update_cached_mode` | 786 | 11 |  |
| fs/inode.rs | `new` | 797 | 28 | yes |
| fs/inode.rs | `with_superblock` | 825 | 27 |  |
| fs/inode.rs | `read_data` | 852 | 19 | yes |
| fs/inode.rs | `append_data` | 871 | 17 | yes |
| fs/inode.rs | `write_data` | 888 | 17 | yes |
| fs/inode.rs | `load_from_bytes` | 905 | 7 | yes |
| fs/inode.rs | `set_ops` | 912 | 5 | yes |
| fs/inode.rs | `set_private_data` | 917 | 5 | yes |
| fs/inode.rs | `get_size` | 922 | 5 | yes |
| fs/inode.rs | `set_size` | 927 | 5 | yes |
| fs/inode.rs | `set_rdev` | 932 | 5 |  |
| fs/inode.rs | `get_rdev` | 937 | 6 |  |
| fs/inode.rs | `inc_ref` | 943 | 5 | yes |
| fs/inode.rs | `dec_ref` | 948 | 5 | yes |
| fs/inode.rs | `get_ref` | 953 | 15 | yes |
| fs/inode.rs | `op_lookup` | 968 | 11 | yes |
| fs/inode.rs | `op_create` | 979 | 11 |  |
| fs/inode.rs | `op_mkdir` | 990 | 11 |  |
| fs/inode.rs | `op_unlink` | 1001 | 11 |  |
| fs/inode.rs | `op_rmdir` | 1012 | 11 |  |
| fs/inode.rs | `op_link` | 1023 | 11 |  |
| fs/inode.rs | `op_symlink` | 1034 | 11 |  |
| fs/inode.rs | `op_rename` | 1045 | 11 | yes |
| fs/inode.rs | `op_readlink` | 1056 | 11 | yes |
| fs/inode.rs | `op_getattr` | 1067 | 15 | yes |
| fs/inode.rs | `op_setattr` | 1082 | 11 | yes |
| fs/inode.rs | `op_permission` | 1093 | 12 |  |
| fs/inode.rs | `make_char_inode` | 1105 | 7 |  |
| fs/inode.rs | `make_reg_inode` | 1112 | 7 |  |
| fs/inode.rs | `make_reg_inode_with_data` | 1119 | 7 |  |
| fs/inode.rs | `make_dir_inode` | 1126 | 5 |  |
| fs/inode.rs | `make_fifo_inode` | 1131 | 27 |  |
| fs/inode.rs | `new` | 1158 | 8 | yes |
| fs/inode.rs | `record_hit` | 1166 | 4 | yes |
| fs/inode.rs | `record_miss` | 1170 | 4 | yes |
| fs/inode.rs | `record_eviction` | 1174 | 4 | yes |
| fs/inode.rs | `get_hit_rate` | 1178 | 25 | yes |
| fs/inode.rs | `clone` | 1203 | 41 | yes |
| fs/inode.rs | `icache_regen` | 1244 | 12 | yes |
| fs/inode.rs | `icache_init` | 1256 | 34 |  |
| fs/inode.rs | `icache_stats_arm` | 1290 | 7 |  |
| fs/inode.rs | `inode_hash` | 1297 | 16 |  |
| fs/inode.rs | `icache_lookup` | 1313 | 40 | yes |
| fs/inode.rs | `icache_add` | 1353 | 54 | yes |
| fs/inode.rs | `icache_evict_lru` | 1407 | 38 |  |
| fs/inode.rs | `icache_remove` | 1445 | 27 | yes |
| fs/inode.rs | `icache_stats` | 1472 | 11 |  |
| fs/inode.rs | `icache_stats_detailed` | 1483 | 17 |  |
| fs/inode.rs | `icache_flush` | 1500 | 18 |  |
| fs/io_completion.rs | `new` | 87 | 21 | yes |
| fs/io_completion.rs | `begin_delivery` | 108 | 10 |  |
| fs/io_completion.rs | `end_delivery` | 118 | 9 |  |
| fs/io_completion.rs | `wait_deliveries` | 127 | 24 |  |
| fs/io_completion.rs | `complete` | 151 | 15 | yes |
| fs/io_completion.rs | `complete_with_token` | 166 | 12 |  |
| fs/io_completion.rs | `delivered_token` | 178 | 9 | yes |
| fs/io_completion.rs | `wait` | 187 | 228 | yes |
| fs/io_completion.rs | `try_wait` | 415 | 9 |  |
| fs/io_completion.rs | `is_done` | 424 | 5 |  |
| fs/io_completion.rs | `reset` | 429 | 9 | yes |
| fs/io_completion.rs | `wait_for_all` | 438 | 10 |  |
| fs/journal_seam.rs | `new` | 71 | 13 | yes |
| fs/journal_seam.rs | `begin_pre_capture` | 84 | 11 |  |
| fs/journal_seam.rs | `take_pre_capture` | 95 | 14 |  |
| fs/journal_seam.rs | `begin_post_capture` | 109 | 9 |  |
| fs/journal_seam.rs | `end_post_capture` | 118 | 8 |  |
| fs/journal_seam.rs | `pre_push` | 126 | 9 | yes |
| fs/journal_seam.rs | `current_handle_raw` | 135 | 26 | yes |
| fs/journal_seam.rs | `empty` | 161 | 4 | yes |
| fs/journal_seam.rs | `get` | 165 | 3 | yes |
| fs/journal_seam.rs | `is_some` | 168 | 3 | yes |
| fs/journal_seam.rs | `set` | 171 | 3 | yes |
| fs/journal_seam.rs | `clear` | 174 | 4 | yes |
| fs/lock_hooks.rs | `inode_open_register` | 19 | 5 | yes |
| fs/lock_hooks.rs | `inode_open_unregister` | 24 | 5 | yes |
| fs/lock_hooks.rs | `flock_release_file` | 29 | 5 | yes |
| fs/lock_hooks.rs | `ofd_release_file` | 34 | 5 | yes |
| fs/lock_hooks.rs | `break_lease` | 39 | 8 | yes |
| fs/lock_hooks.rs | `posix_test_lock` | 47 | 11 | yes |
| fs/lock_hooks.rs | `posix_set_lock` | 58 | 12 | yes |
| fs/lock_hooks.rs | `set_lease` | 70 | 6 | yes |
| fs/lock_hooks.rs | `get_lease` | 76 | 5 | yes |
| fs/lock_hooks.rs | `inode_text_hold_count` | 81 | 3 | yes |
| fs/locks.rs | `snapshot_locks` | 75 | 74 |  |
| fs/locks.rs | `dump_held_locks` | 149 | 47 |  |
| fs/locks.rs | `file_lock_key` | 196 | 11 |  |
| fs/locks.rs | `inode_lock_key` | 207 | 6 |  |
| fs/locks.rs | `current_tgid` | 213 | 39 |  |
| fs/locks.rs | `note_flock_len` | 252 | 8 |  |
| fs/locks.rs | `flock_conflicts` | 260 | 10 |  |
| fs/locks.rs | `flock_lock` | 270 | 73 |  |
| fs/locks.rs | `flock_conflicts_locked` | 343 | 14 |  |
| fs/locks.rs | `flock_release_file` | 357 | 38 | yes |
| fs/locks.rs | `report_pid` | 395 | 52 |  |
| fs/locks.rs | `note_table_len` | 447 | 16 |  |
| fs/locks.rs | `deadlock_check` | 463 | 50 |  |
| fs/locks.rs | `ranges_overlap` | 513 | 7 |  |
| fs/locks.rs | `owner_is_dead` | 520 | 9 |  |
| fs/locks.rs | `reap_stale_locked` | 529 | 19 |  |
| fs/locks.rs | `find_conflict` | 548 | 28 |  |
| fs/locks.rs | `posix_test_lock` | 576 | 21 | yes |
| fs/locks.rs | `apply_owner_lock` | 597 | 71 |  |
| fs/locks.rs | `posix_set_lock` | 668 | 106 | yes |
| fs/locks.rs | `posix_release_on_close` | 774 | 23 |  |
| fs/locks.rs | `ofd_release_file` | 797 | 17 | yes |
| fs/locks.rs | `posix_release_for_file` | 814 | 11 |  |
| fs/locks.rs | `record_lock_count` | 825 | 6 |  |
| fs/locks.rs | `flock_lock_count` | 831 | 54 |  |
| fs/locks.rs | `note_lease_len` | 885 | 23 |  |
| fs/locks.rs | `inode_open_register` | 908 | 13 | yes |
| fs/locks.rs | `inode_open_unregister` | 921 | 38 | yes |
| fs/locks.rs | `inode_write_open_count` | 959 | 15 |  |
| fs/locks.rs | `inode_text_hold_key` | 974 | 5 |  |
| fs/locks.rs | `inode_text_hold_acquire_key` | 979 | 7 |  |
| fs/locks.rs | `inode_text_hold_release` | 986 | 13 |  |
| fs/locks.rs | `inode_text_hold_count` | 999 | 8 | yes |
| fs/locks.rs | `set_lease` | 1007 | 100 | yes |
| fs/locks.rs | `get_lease` | 1107 | 28 | yes |
| fs/locks.rs | `lease_conflicts` | 1135 | 21 |  |
| fs/locks.rs | `break_lease` | 1156 | 72 | yes |
| fs/mod.rs | `read_file_from_rootfs` | 72 | 30 |  |
| fs/mount.rs | `register_mount` | 41 | 20 |  |
| fs/mount.rs | `mounted_as` | 61 | 9 |  |
| fs/mount.rs | `get_global_mounts` | 70 | 14 |  |
| fs/mount.rs | `get_mounts` | 84 | 11 | yes |
| fs/mount.rs | `get_mount_entries` | 95 | 24 |  |
| fs/mount.rs | `new` | 119 | 4 | yes |
| fs/mount.rs | `is_readonly` | 123 | 4 | yes |
| fs/mount.rs | `is_noexec` | 127 | 4 |  |
| fs/mount.rs | `is_nosuid` | 131 | 4 |  |
| fs/mount.rs | `bits` | 135 | 21 | yes |
| fs/mount.rs | `new` | 156 | 12 | yes |
| fs/mount.rs | `get_superblock` | 168 | 4 |  |
| fs/mount.rs | `set_superblock` | 172 | 19 |  |
| fs/mount.rs | `translate_mnt_flags` | 191 | 13 |  |
| fs/mount.rs | `translate_mnt_flags_public` | 204 | 12 |  |
| fs/mount.rs | `do_mount` | 216 | 37 |  |
| fs/mount_table.rs | `register` | 33 | 15 |  |
| fs/mount_table.rs | `lookup` | 48 | 15 | yes |
| fs/mount_table.rs | `register_builtin_handlers` | 63 | 19 |  |
| fs/notify_hooks.rs | `notify` | 22 | 11 | yes |
| fs/notify_hooks.rs | `notify_file` | 33 | 5 | yes |
| fs/notify_hooks.rs | `notify_file_close` | 38 | 5 | yes |
| fs/notify_hooks.rs | `alloc_cookie` | 43 | 3 | yes |
| fs/page_cache.rs | `max_cached_pages` | 105 | 83 | yes |
| fs/page_cache.rs | `dirty_account_transition` | 188 | 14 | yes |
| fs/page_cache.rs | `dirty_account_removed` | 202 | 7 | yes |
| fs/page_cache.rs | `dirty_pages` | 209 | 16 | yes |
| fs/page_cache.rs | `len` | 225 | 47 | yes |
| fs/page_cache.rs | `pcache_reclaim_census_line` | 272 | 29 | yes |
| fs/page_cache.rs | `pcache_dirty_histogram` | 301 | 93 | yes |
| fs/page_cache.rs | `pin_locked` | 394 | 6 | yes |
| fs/page_cache.rs | `unpin_locked` | 400 | 9 | yes |
| fs/page_cache.rs | `count_locked` | 409 | 5 | yes |
| fs/page_cache.rs | `take_released_locked` | 414 | 83 | yes |
| fs/page_cache.rs | `new` | 497 | 39 | yes |
| fs/page_cache.rs | `claim_entry_arc` | 536 | 14 | yes |
| fs/page_cache.rs | `cache_key` | 550 | 6 | yes |
| fs/page_cache.rs | `phys_to_virt_ptr` | 556 | 17 | yes |
| fs/page_cache.rs | `from_phys` | 573 | 8 | yes |
| fs/page_cache.rs | `fill_from` | 581 | 14 | yes |
| fs/page_cache.rs | `copy_frame_from` | 595 | 26 | yes |
| fs/page_cache.rs | `fill_page_from` | 621 | 6 | yes |
| fs/page_cache.rs | `copy_page_page` | 627 | 7 | yes |
| fs/page_cache.rs | `copy_into_page` | 634 | 13 | yes |
| fs/page_cache.rs | `copy_user_into_page` | 647 | 14 | yes |
| fs/page_cache.rs | `new` | 661 | 47 | yes |
| fs/page_cache.rs | `lookup_claimed` | 708 | 39 | yes |
| fs/page_cache.rs | `probe_slot_ptr` | 747 | 24 | yes |
| fs/page_cache.rs | `table_find` | 771 | 23 | yes |
| fs/page_cache.rs | `table_find_ptr` | 794 | 26 | yes |
| fs/page_cache.rs | `table_publish` | 820 | 50 | yes |
| fs/page_cache.rs | `remove_entry` | 870 | 24 | yes |
| fs/page_cache.rs | `park_and_drain` | 894 | 10 | yes |
| fs/page_cache.rs | `flush_parked` | 904 | 24 | yes |
| fs/page_cache.rs | `drain_retired` | 928 | 53 | yes |
| fs/page_cache.rs | `get` | 981 | 35 | yes |
| fs/page_cache.rs | `resident` | 1016 | 17 | yes |
| fs/page_cache.rs | `get_phys` | 1033 | 29 | yes |
| fs/page_cache.rs | `get_stale` | 1062 | 32 | yes |
| fs/page_cache.rs | `get_stale_revive` | 1094 | 47 | yes |
| fs/page_cache.rs | `get_range_into` | 1141 | 19 | yes |
| fs/page_cache.rs | `get_range_with_entry` | 1160 | 66 | yes |
| fs/page_cache.rs | `unpin_contiguous_locked` | 1226 | 61 | yes |
| fs/page_cache.rs | `put_range` | 1287 | 55 | yes |
| fs/page_cache.rs | `put_range_with_entry` | 1342 | 42 | yes |
| fs/page_cache.rs | `put` | 1384 | 49 | yes |
| fs/page_cache.rs | `put_by_key` | 1433 | 39 | yes |
| fs/page_cache.rs | `put_range_by_key` | 1472 | 54 | yes |
| fs/page_cache.rs | `pin_by_key` | 1526 | 48 | yes |
| fs/page_cache.rs | `mark_dirty` | 1574 | 6 | yes |
| fs/page_cache.rs | `mark_dirty_by_key` | 1580 | 20 | yes |
| fs/page_cache.rs | `mark_dirty_pfn` | 1600 | 39 | yes |
| fs/page_cache.rs | `writeback_update_range` | 1639 | 18 | yes |
| fs/page_cache.rs | `writeback_update_range_multi` | 1657 | 18 | yes |
| fs/page_cache.rs | `writeback_update_range_pinned` | 1675 | 64 | yes |
| fs/page_cache.rs | `writeback_update_range_stale` | 1739 | 10 | yes |
| fs/page_cache.rs | `writeback_update_range_impl` | 1749 | 77 | yes |
| fs/page_cache.rs | `writeback_populate_range` | 1826 | 13 | yes |
| fs/page_cache.rs | `writeback_populate_range_multi` | 1839 | 58 | yes |
| fs/page_cache.rs | `collect_dirty_range` | 1897 | 47 | yes |
| fs/page_cache.rs | `dirty_extent_hint` | 1944 | 23 | yes |
| fs/page_cache.rs | `dirty_tail_eof` | 1967 | 23 | yes |
| fs/page_cache.rs | `note_dirty_eof` | 1990 | 41 | yes |
| fs/page_cache.rs | `flush_all_dirty` | 2031 | 78 | yes |
| fs/page_cache.rs | `flush_done` | 2109 | 75 | yes |
| fs/page_cache.rs | `invalidate_inode` | 2184 | 66 | yes |
| fs/page_cache.rs | `invalidate_inode_range` | 2250 | 19 | yes |
| fs/page_cache.rs | `invalidate_inode_span` | 2269 | 71 | yes |
| fs/page_cache.rs | `insert` | 2340 | 22 | yes |
| fs/page_cache.rs | `insert_dirty` | 2362 | 15 | yes |
| fs/page_cache.rs | `insert_dirty_user` | 2377 | 132 | yes |
| fs/page_cache.rs | `insert_ext` | 2509 | 230 | yes |
| fs/page_cache.rs | `insert_batch` | 2739 | 21 | yes |
| fs/page_cache.rs | `insert_dirty_batch` | 2760 | 25 | yes |
| fs/page_cache.rs | `insert_dirty_batch_user` | 2785 | 24 | yes |
| fs/page_cache.rs | `insert_batch_ext` | 2809 | 20 | yes |
| fs/page_cache.rs | `discard_prepared_frame` | 2829 | 398 | yes |
| fs/page_cache.rs | `shrink` | 3227 | 40 | yes |
| fs/page_cache.rs | `collect_wb_work` | 3267 | 67 | yes |
| fs/page_cache.rs | `evict_clean_inode` | 3334 | 85 | yes |
| fs/page_cache.rs | `writeback_recover` | 3419 | 91 | yes |
| fs/page_cache.rs | `shrink_reclaim` | 3510 | 128 | yes |
| fs/page_cache.rs | `evict_one_locked` | 3638 | 245 | yes |
| fs/page_cache.rs | `release_page_frame` | 3883 | 60 | yes |
| fs/page_cache.rs | `get_page_cache` | 3943 | 5 | yes |
| fs/page_cache.rs | `page_cache_total_pages` | 3948 | 8 | yes |
| fs/page_cache.rs | `page_cache_stats` | 3956 | 11 | yes |
| fs/page_cache.rs | `page_cache_cap_pages` | 3967 | 24 | yes |
| fs/page_cache.rs | `shared_file_fault_page` | 3991 | 50 | yes |
| fs/path.rs | `new` | 55 | 8 | yes |
| fs/path.rs | `name` | 63 | 5 | yes |
| fs/path.rs | `is_current` | 68 | 5 |  |
| fs/path.rs | `is_parent` | 73 | 5 |  |
| fs/path.rs | `is_root` | 78 | 5 |  |
| fs/path.rs | `is_empty` | 83 | 13 | yes |
| fs/path.rs | `new` | 96 | 5 | yes |
| fs/path.rs | `is_absolute` | 101 | 5 |  |
| fs/path.rs | `is_empty` | 106 | 5 | yes |
| fs/path.rs | `as_str` | 111 | 5 | yes |
| fs/path.rs | `components` | 116 | 8 |  |
| fs/path.rs | `parent` | 124 | 13 |  |
| fs/path.rs | `file_name` | 137 | 15 |  |
| fs/path.rs | `join` | 152 | 25 | yes |
| fs/path.rs | `next` | 177 | 21 | yes |
| fs/path.rs | `filename_parentname` | 198 | 21 |  |
| fs/path.rs | `path_normalize` | 219 | 71 | yes |
| fs/permission.rs | `generic_permission` | 22 | 81 | yes |
| fs/permission.rs | `inode_permission` | 103 | 17 | yes |
| fs/pidfd.rs | `exit_notify_all_waiters` | 54 | 15 |  |
| fs/pidfd.rs | `pin_leader` | 69 | 24 |  |
| fs/pidfd.rs | `make_pidfd_data` | 93 | 6 | yes |
| fs/pidfd.rs | `free_pidfd_data` | 99 | 12 |  |
| fs/pidfd.rs | `attach` | 111 | 9 | yes |
| fs/pidfd.rs | `data_of` | 120 | 18 |  |
| fs/pidfd.rs | `fdinfo_pid_of` | 138 | 14 |  |
| fs/pidfd.rs | `resolve_alive` | 152 | 14 |  |
| fs/pidfd.rs | `target_exited` | 166 | 14 |  |
| fs/pidfd.rs | `pidfd_poll` | 180 | 9 |  |
| fs/pidfd.rs | `pidfd_close` | 189 | 17 |  |
| fs/pidfd.rs | `pidfd_poll_wait` | 206 | 9 |  |
| fs/pidfd.rs | `pidfd_poll_unwait` | 215 | 9 |  |
| fs/pidfd.rs | `pidfd_dir_close` | 224 | 5 |  |
| fs/pidfd.rs | `pidfd_dir_poll` | 229 | 3 |  |
| fs/procfs/cmdline.rs | `generate` | 17 | 15 |  |
| fs/procfs/cmdline.rs | `get_cmdline` | 32 | 8 | yes |
| fs/procfs/configgz.rs | `crc32_ieee` | 38 | 16 |  |
| fs/procfs/configgz.rs | `generate` | 54 | 20 |  |
| fs/procfs/cpuinfo.rs | `generate` | 63 | 69 |  |
| fs/procfs/diskstats.rs | `generate` | 32 | 65 |  |
| fs/procfs/interrupts.rs | `timer_inc` | 38 | 7 | yes |
| fs/procfs/interrupts.rs | `soft_inc` | 45 | 11 | yes |
| fs/procfs/interrupts.rs | `timer_count` | 56 | 9 | yes |
| fs/procfs/interrupts.rs | `soft_count` | 65 | 10 |  |
| fs/procfs/interrupts.rs | `total_interrupt_count` | 75 | 21 |  |
| fs/procfs/interrupts.rs | `plic_inc` | 96 | 5 |  |
| fs/procfs/interrupts.rs | `plic_count` | 101 | 9 |  |
| fs/procfs/interrupts.rs | `generate` | 110 | 66 |  |
| fs/procfs/kallsyms.rs | `generate` | 45 | 18 |  |
| fs/procfs/loadavg.rs | `update_load_avg` | 73 | 40 | yes |
| fs/procfs/loadavg.rs | `fmt_load` | 113 | 9 |  |
| fs/procfs/loadavg.rs | `generate` | 122 | 26 |  |
| fs/procfs/locks.rs | `generate` | 33 | 6 |  |
| fs/procfs/locks.rs | `render` | 39 | 33 |  |
| fs/procfs/meminfo.rs | `mem_total_kb` | 16 | 5 |  |
| fs/procfs/meminfo.rs | `mem_free_kb` | 21 | 5 |  |
| fs/procfs/meminfo.rs | `mem_available_kb` | 26 | 5 |  |
| fs/procfs/meminfo.rs | `mem_used_kb` | 31 | 5 |  |
| fs/procfs/meminfo.rs | `cached_kb` | 36 | 15 |  |
| fs/procfs/meminfo.rs | `lru_kb` | 51 | 21 |  |
| fs/procfs/meminfo.rs | `swap_kb` | 72 | 9 |  |
| fs/procfs/meminfo.rs | `heap_kb` | 81 | 9 |  |
| fs/procfs/meminfo.rs | `page_cache_ops_line` | 90 | 13 |  |
| fs/procfs/meminfo.rs | `active_inactive_kb` | 103 | 90 |  |
| fs/procfs/meminfo.rs | `generate` | 193 | 4 |  |
| fs/procfs/mod.rs | `attr_file_kind_of` | 97 | 25 |  |
| fs/procfs/mod.rs | `is_fdinfo_dir` | 122 | 6 |  |
| fs/procfs/mod.rs | `fdinfo_file_of` | 128 | 12 |  |
| fs/procfs/mod.rs | `is_task_dir` | 140 | 6 |  |
| fs/procfs/mod.rs | `is_attr_dir` | 146 | 124 |  |
| fs/procfs/mod.rs | `new_dir` | 270 | 23 |  |
| fs/procfs/mod.rs | `new_dynamic_file` | 293 | 23 |  |
| fs/procfs/mod.rs | `new_static_file` | 316 | 24 |  |
| fs/procfs/mod.rs | `new_dynamic_symlink` | 340 | 24 |  |
| fs/procfs/mod.rs | `new_sysctl_file` | 364 | 28 |  |
| fs/procfs/mod.rs | `new_symlink` | 392 | 24 |  |
| fs/procfs/mod.rs | `is_dir` | 416 | 5 |  |
| fs/procfs/mod.rs | `is_file` | 421 | 5 |  |
| fs/procfs/mod.rs | `is_symlink` | 426 | 5 | yes |
| fs/procfs/mod.rs | `get_content` | 431 | 49 |  |
| fs/procfs/mod.rs | `get_link_target` | 480 | 20 |  |
| fs/procfs/mod.rs | `size` | 500 | 9 | yes |
| fs/procfs/mod.rs | `find_child` | 509 | 26 |  |
| fs/procfs/mod.rs | `add_child` | 535 | 6 | yes |
| fs/procfs/mod.rs | `list_children` | 541 | 9 |  |
| fs/procfs/mod.rs | `get` | 550 | 7 | yes |
| fs/procfs/mod.rs | `put` | 557 | 24 | yes |
| fs/procfs/mod.rs | `new` | 581 | 12 | yes |
| fs/procfs/mod.rs | `alloc_ino` | 593 | 5 |  |
| fs/procfs/mod.rs | `init_default_files` | 598 | 101 |  |
| fs/procfs/mod.rs | `init_net_tree` | 699 | 57 |  |
| fs/procfs/mod.rs | `init_sys_tree` | 756 | 318 |  |
| fs/procfs/mod.rs | `init_sysvipc_tree` | 1074 | 21 |  |
| fs/procfs/mod.rs | `create_dynamic_file` | 1095 | 11 |  |
| fs/procfs/mod.rs | `create_static_file` | 1106 | 11 |  |
| fs/procfs/mod.rs | `create_dynamic_symlink` | 1117 | 11 |  |
| fs/procfs/mod.rs | `create_symlink` | 1128 | 11 |  |
| fs/procfs/mod.rs | `lookup` | 1139 | 42 | yes |
| fs/procfs/mod.rs | `read_file` | 1181 | 12 |  |
| fs/procfs/mod.rs | `list_dir` | 1193 | 33 |  |
| fs/procfs/mod.rs | `procfs_mount` | 1226 | 9 |  |
| fs/procfs/mod.rs | `procfs_kill_sb` | 1235 | 8 |  |
| fs/procfs/mod.rs | `get_procfs_sb` | 1243 | 11 |  |
| fs/procfs/mod.rs | `mount_table_regen` | 1254 | 24 |  |
| fs/procfs/mod.rs | `read_file` | 1278 | 119 |  |
| fs/procfs/mod.rs | `is_mounted` | 1397 | 7 |  |
| fs/procfs/mod.rs | `create_root_inode` | 1404 | 25 |  |
| fs/procfs/mod.rs | `list_dir` | 1429 | 5 |  |
| fs/procfs/mod.rs | `exists` | 1434 | 12 |  |
| fs/procfs/mod.rs | `init_procfs` | 1446 | 22 |  |
| fs/procfs/mod.rs | `mount_procfs` | 1468 | 36 |  |
| fs/procfs/mod.rs | `procfs_lookup` | 1504 | 133 |  |
| fs/procfs/mod.rs | `procfs_getattr` | 1637 | 203 |  |
| fs/procfs/mod.rs | `procfs_readlink` | 1840 | 47 |  |
| fs/procfs/mod.rs | `procfs_iget` | 1887 | 289 |  |
| fs/procfs/mod.rs | `procfs_file_read` | 2176 | 22 |  |
| fs/procfs/mod.rs | `procfs_file_write` | 2198 | 91 |  |
| fs/procfs/mod.rs | `procfs_file_lseek` | 2289 | 53 |  |
| fs/procfs/mod.rs | `procfs_file_close` | 2342 | 14 |  |
| fs/procfs/mod.rs | `procfs_setattr` | 2356 | 42 |  |
| fs/procfs/mod.rs | `procfs_file_poll` | 2398 | 28 |  |
| fs/procfs/mod.rs | `proc_piddir_close` | 2426 | 4 |  |
| fs/procfs/mod.rs | `proc_piddir_poll` | 2430 | 13 |  |
| fs/procfs/mod.rs | `procfs_get_file_ops` | 2443 | 33 |  |
| fs/procfs/mod.rs | `procfs_open` | 2476 | 56 |  |
| fs/procfs/mod.rs | `generate_pid_dir_entries` | 2532 | 52 |  |
| fs/procfs/mod.rs | `procfs_readdir` | 2584 | 152 |  |
| fs/procfs/mod.rs | `procfs_ns_dir_lookup` | 2736 | 14 |  |
| fs/procfs/mod.rs | `procfs_ns_dir_iget` | 2750 | 15 |  |
| fs/procfs/mod.rs | `procfs_ns_dir_readdir` | 2765 | 28 |  |
| fs/procfs/mod.rs | `procfs_ns_dir_getattr` | 2793 | 23 |  |
| fs/procfs/mod.rs | `procfs_fd_dir_lookup` | 2816 | 33 |  |
| fs/procfs/mod.rs | `procfs_fd_dir_iget` | 2849 | 44 |  |
| fs/procfs/mod.rs | `generate_slabinfo` | 2893 | 9 |  |
| fs/procfs/mod.rs | `generate_vmallocinfo` | 2902 | 4 |  |
| fs/procfs/mod.rs | `partitions_generate` | 2906 | 24 |  |
| fs/procfs/mod.rs | `mount_proc` | 2930 | 8 |  |
| fs/procfs/mod.rs | `mount_register` | 2938 | 3 |  |
| fs/procfs/mounts.rs | `boot_mounts` | 45 | 57 |  |
| fs/procfs/mounts.rs | `generate` | 102 | 44 |  |
| fs/procfs/mounts.rs | `generate_filesystems` | 146 | 18 |  |
| fs/procfs/mounts.rs | `mount_point_prefix` | 164 | 17 |  |
| fs/procfs/mounts.rs | `mount_id_for` | 181 | 31 |  |
| fs/procfs/mounts.rs | `generate_mountinfo` | 212 | 50 |  |
| fs/procfs/net.rs | `hex_addr_v4` | 27 | 11 |  |
| fs/procfs/net.rs | `hex_port` | 38 | 7 |  |
| fs/procfs/net.rs | `hex_addr_v6` | 45 | 16 |  |
| fs/procfs/net.rs | `dev_rows` | 61 | 11 |  |
| fs/procfs/net.rs | `generate_dev` | 72 | 40 |  |
| fs/procfs/net.rs | `sock_table_header` | 112 | 6 |  |
| fs/procfs/net.rs | `sock_row` | 118 | 21 |  |
| fs/procfs/net.rs | `generate_tcp` | 139 | 21 |  |
| fs/procfs/net.rs | `generate_tcp6` | 160 | 22 |  |
| fs/procfs/net.rs | `generate_udp` | 182 | 21 |  |
| fs/procfs/net.rs | `generate_udp6` | 203 | 25 |  |
| fs/procfs/net.rs | `generate_arp` | 228 | 16 |  |
| fs/procfs/net.rs | `format_mac` | 244 | 8 |  |
| fs/procfs/net.rs | `format_v4` | 252 | 15 |  |
| fs/procfs/net.rs | `generate_route` | 267 | 31 |  |
| fs/procfs/net.rs | `generate_fib_trie` | 298 | 23 |  |
| fs/procfs/net.rs | `generate_sockstat` | 321 | 40 |  |
| fs/procfs/net.rs | `generate_unix` | 361 | 8 |  |
| fs/procfs/pid.rs | `is_pid_dir` | 25 | 8 |  |
| fs/procfs/pid.rs | `is_valid_pid` | 33 | 10 | yes |
| fs/procfs/pid.rs | `parse_pid` | 43 | 22 |  |
| fs/procfs/pid.rs | `task_state_char` | 65 | 13 |  |
| fs/procfs/pid.rs | `task_state_str` | 78 | 22 |  |
| fs/procfs/pid.rs | `task_comm` | 100 | 48 |  |
| fs/procfs/pid.rs | `lookup` | 148 | 4 | yes |
| fs/procfs/pid.rs | `get` | 152 | 8 | yes |
| fs/procfs/pid.rs | `drop` | 160 | 6 | yes |
| fs/procfs/pid.rs | `generate_comm` | 166 | 60 |  |
| fs/procfs/pid.rs | `generate_cgroup` | 226 | 32 |  |
| fs/procfs/pid.rs | `generate_status` | 258 | 200 |  |
| fs/procfs/pid.rs | `read_target_user_mm` | 458 | 48 |  |
| fs/procfs/pid.rs | `environ_access_allowed` | 506 | 13 |  |
| fs/procfs/pid.rs | `generate_cmdline` | 519 | 45 |  |
| fs/procfs/pid.rs | `generate_schedstat` | 564 | 22 |  |
| fs/procfs/pid.rs | `generate_stat` | 586 | 134 |  |
| fs/procfs/pid.rs | `generate_exe_link` | 720 | 24 |  |
| fs/procfs/pid.rs | `generate_cwd_link` | 744 | 24 |  |
| fs/procfs/pid.rs | `generate_environ` | 768 | 45 |  |
| fs/procfs/pid.rs | `generate_smaps` | 813 | 83 |  |
| fs/procfs/pid.rs | `re_header` | 896 | 14 |  |
| fs/procfs/pid.rs | `generate_maps` | 910 | 168 |  |
| fs/procfs/pid.rs | `get_fd_file` | 1078 | 18 | yes |
| fs/procfs/pid.rs | `list_fds` | 1096 | 31 | yes |
| fs/procfs/pid.rs | `get_fd_path` | 1127 | 30 |  |
| fs/procfs/pid.rs | `generate_fd_link` | 1157 | 32 |  |
| fs/procfs/pid.rs | `generate_oom_score` | 1189 | 39 |  |
| fs/procfs/pid.rs | `generate_oom_score_adj` | 1228 | 46 |  |
| fs/procfs/pid.rs | `trim_label` | 1274 | 39 |  |
| fs/procfs/pid.rs | `attr_get` | 1313 | 36 |  |
| fs/procfs/pid.rs | `attr_set` | 1349 | 11 |  |
| fs/procfs/pid.rs | `attr_label_get` | 1360 | 5 |  |
| fs/procfs/pid.rs | `attr_label_set` | 1365 | 9 |  |
| fs/procfs/pid.rs | `generate_fdinfo` | 1374 | 49 |  |
| fs/procfs/procfile.rs | `render_kv` | 67 | 19 |  |
| fs/procfs/procfile.rs | `render_pad` | 86 | 28 |  |
| fs/procfs/procfile.rs | `render_kvtab` | 114 | 15 |  |
| fs/procfs/procfile.rs | `render_kvtab_pad` | 129 | 22 |  |
| fs/procfs/procfile.rs | `render_cols` | 151 | 86 |  |
| fs/procfs/schedstat.rs | `note_yield` | 52 | 7 |  |
| fs/procfs/schedstat.rs | `note_sched_run` | 59 | 7 | yes |
| fs/procfs/schedstat.rs | `generate` | 66 | 53 |  |
| fs/procfs/self_proc.rs | `get_self_link` | 16 | 7 |  |
| fs/procfs/self_proc.rs | `generate` | 23 | 9 |  |
| fs/procfs/self_proc.rs | `get_thread_self_link` | 32 | 10 |  |
| fs/procfs/softirqs.rs | `generate` | 36 | 26 |  |
| fs/procfs/softirqs.rs | `total_raised` | 62 | 13 |  |
| fs/procfs/softirqs.rs | `raised_count` | 75 | 3 | yes |
| fs/procfs/stat.rs | `new` | 61 | 30 | yes |
| fs/procfs/stat.rs | `charge_tick` | 91 | 24 | yes |
| fs/procfs/stat.rs | `note_switch` | 115 | 22 | yes |
| fs/procfs/stat.rs | `note_fork` | 137 | 5 |  |
| fs/procfs/stat.rs | `total_context_switches` | 142 | 5 |  |
| fs/procfs/stat.rs | `total_forks` | 147 | 5 |  |
| fs/procfs/stat.rs | `procs_running` | 152 | 5 |  |
| fs/procfs/stat.rs | `procs_blocked` | 157 | 19 |  |
| fs/procfs/stat.rs | `btime` | 176 | 14 |  |
| fs/procfs/stat.rs | `generate` | 190 | 138 |  |
| fs/procfs/swaps.rs | `generate` | 20 | 20 |  |
| fs/procfs/sysctl.rs | `new` | 84 | 11 | yes |
| fs/procfs/sysctl.rs | `as_bytes` | 95 | 13 | yes |
| fs/procfs/sysctl.rs | `generate_hostname` | 108 | 8 |  |
| fs/procfs/sysctl.rs | `generate_domainname` | 116 | 8 |  |
| fs/procfs/sysctl.rs | `generate_osrelease` | 124 | 4 |  |
| fs/procfs/sysctl.rs | `generate_pid_max` | 128 | 4 |  |
| fs/procfs/sysctl.rs | `generate_ostype` | 132 | 12 |  |
| fs/procfs/sysctl.rs | `generate_tainted` | 144 | 9 |  |
| fs/procfs/sysctl.rs | `generate_panic_on_oops` | 153 | 12 |  |
| fs/procfs/sysctl.rs | `generate_cov_dump` | 165 | 5 |  |
| fs/procfs/sysctl.rs | `write_cov_dump` | 170 | 4 |  |
| fs/procfs/sysctl.rs | `write_panic_on_oops` | 174 | 14 |  |
| fs/procfs/sysctl.rs | `generate_sched_schedstats` | 188 | 8 |  |
| fs/procfs/sysctl.rs | `write_sched_schedstats` | 196 | 12 |  |
| fs/procfs/sysctl.rs | `generate_cap_last_cap` | 208 | 4 |  |
| fs/procfs/sysctl.rs | `generate_overcommit_memory` | 212 | 4 |  |
| fs/procfs/sysctl.rs | `generate_file_max` | 216 | 4 |  |
| fs/procfs/sysctl.rs | `generate_pipe_user_pages_soft` | 220 | 4 |  |
| fs/procfs/sysctl.rs | `generate_pipe_user_pages_hard` | 224 | 9 |  |
| fs/procfs/sysctl.rs | `generate_pipe_max_size` | 233 | 4 |  |
| fs/procfs/sysctl.rs | `write_pipe_max_size` | 237 | 17 |  |
| fs/procfs/sysctl.rs | `generate_lease_break_time` | 254 | 8 |  |
| fs/procfs/sysctl.rs | `write_lease_break_time` | 262 | 19 |  |
| fs/procfs/sysctl.rs | `parse_u64` | 281 | 20 |  |
| fs/procfs/sysctl.rs | `write_hostname` | 301 | 18 |  |
| fs/procfs/sysctl.rs | `write_domainname` | 319 | 18 |  |
| fs/procfs/sysctl.rs | `write_pid_max` | 337 | 15 |  |
| fs/procfs/sysctl.rs | `write_overcommit_memory` | 352 | 13 |  |
| fs/procfs/sysctl.rs | `write_file_max` | 365 | 10 |  |
| fs/procfs/sysctl.rs | `write_pipe_user_pages_soft` | 375 | 10 |  |
| fs/procfs/sysctl.rs | `write_pipe_user_pages_hard` | 385 | 12 |  |
| fs/procfs/sysctl.rs | `generate_msgmni` | 397 | 6 |  |
| fs/procfs/sysctl.rs | `write_msgmni` | 403 | 14 |  |
| fs/procfs/sysctl.rs | `generate_threads_max` | 417 | 11 |  |
| fs/procfs/sysctl.rs | `generate_printk` | 428 | 8 |  |
| fs/procfs/sysctl.rs | `write_printk` | 436 | 17 |  |
| fs/procfs/sysctl.rs | `generate_printk_devkmsg` | 453 | 11 |  |
| fs/procfs/sysctl.rs | `write_printk_devkmsg` | 464 | 17 |  |
| fs/procfs/sysctl.rs | `generate_shmmax` | 481 | 4 |  |
| fs/procfs/sysctl.rs | `write_shmmax` | 485 | 12 |  |
| fs/procfs/sysctl.rs | `generate_shmmni` | 497 | 4 |  |
| fs/procfs/sysctl.rs | `write_shmmni` | 501 | 13 |  |
| fs/procfs/sysctl.rs | `generate_shmall` | 514 | 4 |  |
| fs/procfs/sysctl.rs | `write_shmall` | 518 | 11 |  |
| fs/procfs/sysctl.rs | `generate_mq_queues_max` | 529 | 4 |  |
| fs/procfs/sysctl.rs | `write_mq_queues_max` | 533 | 13 |  |
| fs/procfs/sysctl.rs | `generate_mq_msg_max` | 546 | 5 |  |
| fs/procfs/sysctl.rs | `generate_mq_msgsize_max` | 551 | 5 |  |
| fs/procfs/sysctl.rs | `generate_msgmax` | 556 | 5 |  |
| fs/procfs/sysctl.rs | `generate_msgmnb` | 561 | 4 |  |
| fs/procfs/sysctl.rs | `generate_core_pattern` | 565 | 6 |  |
| fs/procfs/sysctl.rs | `write_core_pattern` | 571 | 30 |  |
| fs/procfs/sysctl.rs | `generate_inotify_max_queued_events` | 601 | 4 |  |
| fs/procfs/sysctl.rs | `write_inotify_max_queued_events` | 605 | 20 |  |
| fs/procfs/sysctl.rs | `generate_inotify_max_user_instances` | 625 | 4 |  |
| fs/procfs/sysctl.rs | `write_inotify_max_user_instances` | 629 | 15 |  |
| fs/procfs/sysctl.rs | `generate_inotify_max_user_watches` | 644 | 4 |  |
| fs/procfs/sysctl.rs | `write_inotify_max_user_watches` | 648 | 14 |  |
| fs/procfs/sysctl.rs | `generate_ns_last_pid` | 662 | 4 |  |
| fs/procfs/sysctl.rs | `write_ns_last_pid` | 666 | 30 |  |
| fs/procfs/sysctl.rs | `generate_perf_event_paranoid` | 696 | 3 |  |
| fs/procfs/sysctl.rs | `write_perf_event_paranoid` | 699 | 9 |  |
| fs/procfs/sysctl.rs | `generate_perf_max_sample_rate` | 708 | 3 |  |
| fs/procfs/sysctl.rs | `write_perf_max_sample_rate` | 711 | 9 |  |
| fs/procfs/sysctl.rs | `generate_perf_cpu_time_max_percent` | 720 | 3 |  |
| fs/procfs/sysctl.rs | `write_perf_cpu_time_max_percent` | 723 | 9 |  |
| fs/procfs/sysctl.rs | `generate_perf_event_mlock_kb` | 732 | 3 |  |
| fs/procfs/sysctl.rs | `write_perf_event_mlock_kb` | 735 | 19 |  |
| fs/procfs/sysctl.rs | `generate_key_quota` | 754 | 4 |  |
| fs/procfs/sysctl.rs | `write_key_quota` | 758 | 10 |  |
| fs/procfs/sysctl.rs | `generate_root_maxkeys` | 768 | 3 |  |
| fs/procfs/sysctl.rs | `write_root_maxkeys` | 771 | 3 |  |
| fs/procfs/sysctl.rs | `generate_root_maxbytes` | 774 | 3 |  |
| fs/procfs/sysctl.rs | `write_root_maxbytes` | 777 | 3 |  |
| fs/procfs/sysctl.rs | `generate_maxkeys` | 780 | 3 |  |
| fs/procfs/sysctl.rs | `write_maxkeys` | 783 | 3 |  |
| fs/procfs/sysctl.rs | `generate_maxbytes` | 786 | 3 |  |
| fs/procfs/sysctl.rs | `write_maxbytes` | 789 | 14 |  |
| fs/procfs/sysctl.rs | `format_uuid_v4` | 803 | 12 |  |
| fs/procfs/sysctl.rs | `generate_boot_id` | 815 | 12 |  |
| fs/procfs/sysctl.rs | `generate_uuid` | 827 | 5 |  |
| fs/procfs/sysrq.rs | `run_command` | 30 | 101 |  |
| fs/procfs/sysrq.rs | `signal_all_except_init` | 131 | 18 |  |
| fs/procfs/sysrq.rs | `dump_all_cpu_stacks` | 149 | 25 |  |
| fs/procfs/sysrq.rs | `print_task_stack_words` | 174 | 45 |  |
| fs/procfs/sysrq.rs | `dump_blocked_tasks` | 219 | 27 |  |
| fs/procfs/sysrq.rs | `print_help` | 246 | 20 |  |
| fs/procfs/sysrq.rs | `write_trigger` | 266 | 23 |  |
| fs/procfs/timer_list.rs | `generate` | 53 | 62 |  |
| fs/procfs/uptime.rs | `account_idle_cycles` | 23 | 7 | yes |
| fs/procfs/uptime.rs | `total_idle_cycles` | 30 | 12 |  |
| fs/procfs/uptime.rs | `generate` | 42 | 24 |  |
| fs/procfs/uptime.rs | `read_time_cycles` | 66 | 5 |  |
| fs/procfs/uptime.rs | `boot_time_cycles` | 71 | 4 |  |
| fs/procfs/version.rs | `generate` | 24 | 13 |  |
| fs/procfs/version.rs | `get_version_string` | 37 | 5 |  |
| fs/procfs/version.rs | `get_release_string` | 42 | 4 |  |
| fs/procfs/vmstat.rs | `zone_name` | 33 | 12 |  |
| fs/procfs/vmstat.rs | `nr_free_pages` | 45 | 93 |  |
| fs/procfs/vmstat.rs | `generate` | 138 | 10 |  |
| fs/procfs/vmstat.rs | `generate_buddyinfo` | 148 | 26 |  |
| fs/pty.rs | `new` | 98 | 8 | yes |
| fs/pty.rs | `len` | 106 | 4 | yes |
| fs/pty.rs | `space` | 110 | 5 |  |
| fs/pty.rs | `write_exact` | 115 | 12 |  |
| fs/pty.rs | `read` | 127 | 39 | yes |
| fs/pty.rs | `new` | 166 | 15 | yes |
| fs/pty.rs | `master_rx_len` | 181 | 4 |  |
| fs/pty.rs | `master_rx_space` | 185 | 4 |  |
| fs/pty.rs | `master_rx_read` | 189 | 4 |  |
| fs/pty.rs | `master_rx_write_exact` | 193 | 8 |  |
| fs/pty.rs | `pty_output` | 201 | 21 |  |
| fs/pty.rs | `alloc_pty` | 222 | 33 |  |
| fs/pty.rs | `destroy_pair` | 255 | 16 |  |
| fs/pty.rs | `devfs_register` | 271 | 7 |  |
| fs/pty.rs | `devfs_add_slave_node` | 278 | 14 |  |
| fs/pty.rs | `devfs_remove_slave_node` | 292 | 15 |  |
| fs/pty.rs | `evict_pts_dentry` | 307 | 11 |  |
| fs/pty.rs | `ptmx_open` | 318 | 13 |  |
| fs/pty.rs | `slave_open` | 331 | 28 |  |
| fs/pty.rs | `pair_of` | 359 | 9 |  |
| fs/pty.rs | `pair_arc` | 368 | 10 |  |
| fs/pty.rs | `take_pair_arc` | 378 | 10 |  |
| fs/pty.rs | `nonblock` | 388 | 4 |  |
| fs/pty.rs | `send_sigpipe` | 392 | 10 |  |
| fs/pty.rs | `ptmx_read` | 402 | 60 |  |
| fs/pty.rs | `ptmx_write` | 462 | 19 |  |
| fs/pty.rs | `ptmx_poll` | 481 | 21 |  |
| fs/pty.rs | `ptmx_close` | 502 | 42 |  |
| fs/pty.rs | `pty_slave_read` | 544 | 11 |  |
| fs/pty.rs | `pty_slave_write` | 555 | 88 |  |
| fs/pty.rs | `pty_slave_poll` | 643 | 23 |  |
| fs/pty.rs | `pty_slave_close` | 666 | 44 |  |
| fs/pty.rs | `copy_out` | 710 | 7 | yes |
| fs/pty.rs | `copy_in` | 717 | 6 |  |
| fs/pty.rs | `pty_ioctl` | 723 | 265 |  |
| fs/readahead.rs | `new` | 53 | 12 | yes |
| fs/readahead.rs | `on_read` | 65 | 40 |  |
| fs/rootfs.rs | `new` | 53 | 30 | yes |
| fs/rootfs.rs | `path_cache_init` | 83 | 28 |  |
| fs/rootfs.rs | `stamp_tree` | 111 | 8 |  |
| fs/rootfs.rs | `path_cache_lookup` | 119 | 21 |  |
| fs/rootfs.rs | `path_cache_add` | 140 | 18 |  |
| fs/rootfs.rs | `path_cache_flush` | 158 | 12 |  |
| fs/rootfs.rs | `rootfs_path_hash` | 170 | 11 |  |
| fs/rootfs.rs | `wall_secs` | 181 | 5 |  |
| fs/rootfs.rs | `rootfs_count_links` | 186 | 13 |  |
| fs/rootfs.rs | `count_links_in` | 199 | 16 |  |
| fs/rootfs.rs | `rootfs_subtree_contains` | 215 | 14 |  |
| fs/rootfs.rs | `get_rootfs_sb` | 229 | 12 |  |
| fs/rootfs.rs | `get_rootfs_sb_ref` | 241 | 4 |  |
| fs/rootfs.rs | `get_root_mount` | 245 | 70 |  |
| fs/rootfs.rs | `new` | 315 | 21 | yes |
| fs/rootfs.rs | `name` | 336 | 7 | yes |
| fs/rootfs.rs | `new_dir` | 343 | 5 |  |
| fs/rootfs.rs | `new_file` | 348 | 7 |  |
| fs/rootfs.rs | `new_symlink` | 355 | 7 |  |
| fs/rootfs.rs | `get` | 362 | 5 | yes |
| fs/rootfs.rs | `put` | 367 | 7 | yes |
| fs/rootfs.rs | `add_child` | 374 | 11 | yes |
| fs/rootfs.rs | `remove_child` | 385 | 15 | yes |
| fs/rootfs.rs | `set_name` | 400 | 6 |  |
| fs/rootfs.rs | `rename_child` | 406 | 11 |  |
| fs/rootfs.rs | `find_child` | 417 | 12 |  |
| fs/rootfs.rs | `list_children` | 429 | 7 |  |
| fs/rootfs.rs | `is_dir` | 436 | 5 |  |
| fs/rootfs.rs | `is_file` | 441 | 5 |  |
| fs/rootfs.rs | `is_symlink` | 446 | 5 | yes |
| fs/rootfs.rs | `get_link_target` | 451 | 5 |  |
| fs/rootfs.rs | `read_data` | 456 | 16 | yes |
| fs/rootfs.rs | `write_data` | 472 | 30 | yes |
| fs/rootfs.rs | `new` | 502 | 17 | yes |
| fs/rootfs.rs | `get_root` | 519 | 6 | yes |
| fs/rootfs.rs | `alloc_ino` | 525 | 5 |  |
| fs/rootfs.rs | `create_file` | 530 | 44 |  |
| fs/rootfs.rs | `create_dir` | 574 | 60 |  |
| fs/rootfs.rs | `link` | 634 | 89 |  |
| fs/rootfs.rs | `lookup` | 723 | 44 | yes |
| fs/rootfs.rs | `lookup_walk` | 767 | 38 |  |
| fs/rootfs.rs | `list_dir` | 805 | 12 |  |
| fs/rootfs.rs | `mkdir` | 817 | 43 |  |
| fs/rootfs.rs | `unlink` | 860 | 52 |  |
| fs/rootfs.rs | `rmdir` | 912 | 57 |  |
| fs/rootfs.rs | `rename` | 969 | 111 |  |
| fs/rootfs.rs | `symlink` | 1080 | 44 |  |
| fs/rootfs.rs | `readlink` | 1124 | 22 |  |
| fs/rootfs.rs | `follow_link_internal` | 1146 | 40 |  |
| fs/rootfs.rs | `lookup_follow` | 1186 | 59 |  |
| fs/rootfs.rs | `rootfs_mount` | 1245 | 17 |  |
| fs/rootfs.rs | `init_rootfs` | 1262 | 42 |  |
| fs/rootfs.rs | `get_root_node` | 1304 | 8 |  |
| fs/rootfs.rs | `get_rootfs` | 1312 | 6 |  |
| fs/rootfs.rs | `create_root_inode` | 1318 | 24 |  |
| fs/rootfs.rs | `rootfs_lookup` | 1342 | 17 |  |
| fs/rootfs.rs | `rootfs_mkdir` | 1359 | 39 |  |
| fs/rootfs.rs | `rootfs_unlink` | 1398 | 41 |  |
| fs/rootfs.rs | `rootfs_rmdir` | 1439 | 45 |  |
| fs/rootfs.rs | `rootfs_create` | 1484 | 38 |  |
| fs/rootfs.rs | `rootfs_symlink` | 1522 | 36 |  |
| fs/rootfs.rs | `rootfs_link` | 1558 | 57 |  |
| fs/rootfs.rs | `rootfs_rename` | 1615 | 62 |  |
| fs/rootfs.rs | `rootfs_readlink` | 1677 | 23 |  |
| fs/rootfs.rs | `rootfs_setattr` | 1700 | 44 |  |
| fs/rootfs.rs | `rootfs_getattr` | 1744 | 33 |  |
| fs/rootfs.rs | `rootfs_file_read` | 1777 | 25 |  |
| fs/rootfs.rs | `rootfs_file_write` | 1802 | 23 |  |
| fs/rootfs.rs | `rootfs_file_lseek` | 1825 | 23 |  |
| fs/rootfs.rs | `rootfs_file_close` | 1848 | 14 |  |
| fs/rootfs.rs | `rootfs_get_file_ops` | 1862 | 12 |  |
| fs/rootfs.rs | `rootfs_readdir` | 1874 | 30 |  |
| fs/rootfs.rs | `rootfs_destroy_inode` | 1904 | 39 |  |
| fs/rootfs.rs | `rootfs_iget` | 1943 | 27 |  |
| fs/rootfs.rs | `rootfs_node_size` | 1970 | 12 |  |
| fs/stat.rs | `new` | 91 | 24 | yes |
| fs/stat.rs | `set_regular_file` | 115 | 5 | yes |
| fs/stat.rs | `set_directory` | 120 | 5 |  |
| fs/stat.rs | `set_char_device` | 125 | 5 |  |
| fs/stat.rs | `set_block_device` | 130 | 5 |  |
| fs/stat.rs | `set_fifo` | 135 | 5 |  |
| fs/stat.rs | `set_symlink` | 140 | 5 |  |
| fs/stat.rs | `set_socket` | 145 | 5 |  |
| fs/stat.rs | `is_regular_file` | 150 | 5 | yes |
| fs/stat.rs | `is_directory` | 155 | 5 | yes |
| fs/stat.rs | `is_char_device` | 160 | 5 | yes |
| fs/stat.rs | `is_block_device` | 165 | 5 | yes |
| fs/stat.rs | `is_fifo` | 170 | 5 | yes |
| fs/stat.rs | `is_symlink` | 175 | 5 | yes |
| fs/stat.rs | `is_socket` | 180 | 5 |  |
| fs/stat.rs | `set_mode` | 185 | 8 |  |
| fs/stat.rs | `get_mode` | 193 | 6 |  |
| fs/stat.rs | `default` | 199 | 38 | yes |
| fs/stat.rs | `from` | 237 | 26 | yes |
| fs/stat.rs | `user_stat_size` | 263 | 10 |  |
| fs/stat.rs | `copy_stat_to_user` | 273 | 94 |  |
| fs/stat.rs | `new` | 367 | 29 | yes |
| fs/stat.rs | `default` | 396 | 5 | yes |
| fs/superblock.rs | `new` | 45 | 4 | yes |
| fs/superblock.rs | `is_readonly` | 49 | 4 | yes |
| fs/superblock.rs | `is_active` | 53 | 4 |  |
| fs/superblock.rs | `bits` | 57 | 32 | yes |
| fs/superblock.rs | `new` | 89 | 23 | yes |
| fs/superblock.rs | `set_type` | 112 | 5 |  |
| fs/superblock.rs | `set_fs_info` | 117 | 5 |  |
| fs/superblock.rs | `set_flags` | 122 | 18 | yes |
| fs/superblock.rs | `new` | 140 | 33 | yes |
| fs/superblock.rs | `new` | 173 | 16 | yes |
| fs/superblock.rs | `mount_fs` | 189 | 18 |  |
| fs/superblock.rs | `kill_super` | 207 | 16 |  |
| fs/superblock.rs | `new` | 223 | 8 | yes |
| fs/superblock.rs | `register` | 231 | 16 |  |
| fs/superblock.rs | `unregister` | 247 | 18 | yes |
| fs/superblock.rs | `get` | 265 | 17 | yes |
| fs/superblock.rs | `register_filesystem` | 282 | 4 |  |
| fs/superblock.rs | `unregister_filesystem` | 286 | 4 |  |
| fs/superblock.rs | `get_fs_type` | 290 | 4 |  |
| fs/superblock.rs | `do_mount` | 294 | 20 |  |
| fs/superblock.rs | `do_umount` | 314 | 15 |  |
| fs/sysfs.rs | `subsystem` | 121 | 66 |  |
| fs/sysfs.rs | `new_root` | 187 | 15 |  |
| fs/sysfs.rs | `new` | 202 | 16 | yes |
| fs/sysfs.rs | `add_child` | 218 | 10 | yes |
| fs/sysfs.rs | `remove_child` | 228 | 5 | yes |
| fs/sysfs.rs | `find_child` | 233 | 6 |  |
| fs/sysfs.rs | `set_attr` | 239 | 18 |  |
| fs/sysfs.rs | `find_attr` | 257 | 8 |  |
| fs/sysfs.rs | `attr_content` | 265 | 11 |  |
| fs/sysfs.rs | `path` | 276 | 32 | yes |
| fs/sysfs.rs | `alloc_ino` | 308 | 11 |  |
| fs/sysfs.rs | `sysfs_root` | 319 | 29 |  |
| fs/sysfs.rs | `coldboot_uevent_replay` | 348 | 27 |  |
| fs/sysfs.rs | `device_add_payload` | 375 | 21 |  |
| fs/sysfs.rs | `lookup_path` | 396 | 13 |  |
| fs/sysfs.rs | `mk_dir` | 409 | 7 |  |
| fs/sysfs.rs | `mk_link` | 416 | 7 |  |
| fs/sysfs.rs | `attr_ro` | 423 | 8 |  |
| fs/sysfs.rs | `attr_rw` | 431 | 14 |  |
| fs/sysfs.rs | `add_uevent_attr` | 445 | 71 |  |
| fs/sysfs.rs | `parse_uevent_action` | 516 | 23 |  |
| fs/sysfs.rs | `cpu_range_str` | 539 | 9 |  |
| fs/sysfs.rs | `possible_cpus` | 548 | 5 |  |
| fs/sysfs.rs | `add_net_device` | 553 | 76 |  |
| fs/sysfs.rs | `subsystem_link_target` | 629 | 16 |  |
| fs/sysfs.rs | `ensure_devices_chain` | 645 | 52 |  |
| fs/sysfs.rs | `register_block_disk_at` | 697 | 114 |  |
| fs/sysfs.rs | `unregister_block_disk` | 811 | 16 |  |
| fs/sysfs.rs | `find_disk` | 827 | 88 |  |
| fs/sysfs.rs | `dev_env` | 915 | 30 |  |
| fs/sysfs.rs | `add_env_uevent_attr` | 945 | 47 |  |
| fs/sysfs.rs | `register_input_event` | 992 | 109 |  |
| fs/sysfs.rs | `unregister_input_event` | 1101 | 52 |  |
| fs/sysfs.rs | `register_drm_device` | 1153 | 131 |  |
| fs/sysfs.rs | `add_tty_device` | 1284 | 30 |  |
| fs/sysfs.rs | `build_tree` | 1314 | 462 |  |
| fs/sysfs.rs | `started_cpus` | 1776 | 19 |  |
| fs/sysfs.rs | `push_env` | 1795 | 17 |  |
| fs/sysfs.rs | `uevent_send_full` | 1812 | 15 |  |
| fs/sysfs.rs | `uevent_payload_for` | 1827 | 24 |  |
| fs/sysfs.rs | `subsystem_from_devpath` | 1851 | 16 |  |
| fs/sysfs.rs | `uevent_send` | 1867 | 7 |  |
| fs/sysfs.rs | `kobject_uevent` | 1874 | 25 |  |
| fs/sysfs.rs | `netdev_uevent` | 1899 | 7 |  |
| fs/sysfs.rs | `uevent_seqnum` | 1906 | 31 |  |
| fs/sysfs.rs | `sysfs_mount_cb` | 1937 | 12 |  |
| fs/sysfs.rs | `sysfs_kill_sb` | 1949 | 5 |  |
| fs/sysfs.rs | `get_sysfs_sb` | 1954 | 6 |  |
| fs/sysfs.rs | `init_sysfs` | 1960 | 27 |  |
| fs/sysfs.rs | `mount_sysfs` | 1987 | 22 |  |
| fs/sysfs.rs | `is_mounted` | 2009 | 5 |  |
| fs/sysfs.rs | `create_root_inode` | 2014 | 52 |  |
| fs/sysfs.rs | `entry_of` | 2066 | 12 |  |
| fs/sysfs.rs | `resolve_child` | 2078 | 25 |  |
| fs/sysfs.rs | `sysfs_lookup` | 2103 | 10 |  |
| fs/sysfs.rs | `sysfs_iget` | 2113 | 31 |  |
| fs/sysfs.rs | `sysfs_getattr` | 2144 | 47 |  |
| fs/sysfs.rs | `sysfs_setattr` | 2191 | 40 |  |
| fs/sysfs.rs | `sysfs_readlink` | 2231 | 16 |  |
| fs/sysfs.rs | `sysfs_readdir` | 2247 | 36 |  |
| fs/sysfs.rs | `sysfs_open` | 2283 | 24 |  |
| fs/sysfs.rs | `sysfs_get_file_ops` | 2307 | 12 |  |
| fs/sysfs.rs | `sysfs_destroy_inode` | 2319 | 11 |  |
| fs/sysfs.rs | `sysfs_mkdir` | 2330 | 53 |  |
| fs/sysfs.rs | `sysfs_file_read` | 2383 | 19 |  |
| fs/sysfs.rs | `sysfs_file_write` | 2402 | 29 |  |
| fs/sysfs.rs | `sysfs_file_lseek` | 2431 | 21 |  |
| fs/sysfs.rs | `sysfs_file_close` | 2452 | 14 |  |
| fs/sysfs.rs | `sysfs_file_poll` | 2466 | 21 |  |
| fs/sysfs.rs | `mount_sys_instance` | 2487 | 11 |  |
| fs/sysfs.rs | `mount_pstore` | 2498 | 14 |  |
| fs/sysfs.rs | `mount_register` | 2512 | 4 |  |
| fs/tmpfs.rs | `wall_secs` | 63 | 22 |  |
| fs/tmpfs.rs | `new` | 85 | 11 | yes |
| fs/tmpfs.rs | `drop` | 96 | 78 | yes |
| fs/tmpfs.rs | `new` | 174 | 36 | yes |
| fs/tmpfs.rs | `name` | 210 | 7 | yes |
| fs/tmpfs.rs | `set_name` | 217 | 5 |  |
| fs/tmpfs.rs | `is_dir` | 222 | 4 |  |
| fs/tmpfs.rs | `is_file` | 226 | 4 |  |
| fs/tmpfs.rs | `is_symlink` | 230 | 4 | yes |
| fs/tmpfs.rs | `add_child` | 234 | 4 | yes |
| fs/tmpfs.rs | `remove_child` | 238 | 10 | yes |
| fs/tmpfs.rs | `find_child` | 248 | 5 |  |
| fs/tmpfs.rs | `list_children` | 253 | 4 |  |
| fs/tmpfs.rs | `file_size` | 257 | 5 |  |
| fs/tmpfs.rs | `page_count` | 262 | 19 |  |
| fs/tmpfs.rs | `read_at` | 281 | 62 | yes |
| fs/tmpfs.rs | `write_at` | 343 | 110 | yes |
| fs/tmpfs.rs | `fallocate_alloc` | 453 | 32 | yes |
| fs/tmpfs.rs | `punch_hole` | 485 | 63 | yes |
| fs/tmpfs.rs | `truncate` | 548 | 72 | yes |
| fs/tmpfs.rs | `reserve_page` | 620 | 50 |  |
| fs/tmpfs.rs | `new` | 670 | 32 | yes |
| fs/tmpfs.rs | `stamp_final_address` | 702 | 8 |  |
| fs/tmpfs.rs | `alloc_ino` | 710 | 5 |  |
| fs/tmpfs.rs | `new_node` | 715 | 27 |  |
| fs/tmpfs.rs | `parse_size_pages` | 742 | 67 |  |
| fs/tmpfs.rs | `create_mount_instance` | 809 | 19 |  |
| fs/tmpfs.rs | `tag_mount_instance_type` | 828 | 8 |  |
| fs/tmpfs.rs | `tmpfs_make_inode` | 836 | 24 |  |
| fs/tmpfs.rs | `tmpfs_mount` | 860 | 17 |  |
| fs/tmpfs.rs | `ensure_registered` | 877 | 13 |  |
| fs/tmpfs.rs | `node_of` | 890 | 10 |  |
| fs/tmpfs.rs | `tmpfs_count_links` | 900 | 1 |  |
| fs/tmpfs.rs | `walk` | 901 | 15 | yes |
| fs/tmpfs.rs | `tmpfs_subtree_contains` | 916 | 15 |  |
| fs/tmpfs.rs | `tmpfs_lookup` | 931 | 12 |  |
| fs/tmpfs.rs | `tmpfs_create` | 943 | 20 |  |
| fs/tmpfs.rs | `tmpfs_mkdir` | 963 | 19 |  |
| fs/tmpfs.rs | `tmpfs_symlink` | 982 | 23 |  |
| fs/tmpfs.rs | `tmpfs_mknod` | 1005 | 43 |  |
| fs/tmpfs.rs | `tmpfs_link` | 1048 | 43 |  |
| fs/tmpfs.rs | `tmpfs_unlink` | 1091 | 24 |  |
| fs/tmpfs.rs | `tmpfs_rmdir` | 1115 | 27 |  |
| fs/tmpfs.rs | `tmpfs_rename` | 1142 | 46 |  |
| fs/tmpfs.rs | `tmpfs_readlink` | 1188 | 19 |  |
| fs/tmpfs.rs | `tmpfs_getattr` | 1207 | 34 |  |
| fs/tmpfs.rs | `tmpfs_setattr` | 1241 | 70 |  |
| fs/tmpfs.rs | `tmpfs_readdir` | 1311 | 40 |  |
| fs/tmpfs.rs | `tmpfs_iget` | 1351 | 13 |  |
| fs/tmpfs.rs | `tmpfs_destroy_inode` | 1364 | 20 |  |
| fs/tmpfs.rs | `tmpfs_file_read` | 1384 | 53 |  |
| fs/tmpfs.rs | `tmpfs_file_write` | 1437 | 51 |  |
| fs/tmpfs.rs | `tmpfs_file_lseek` | 1488 | 22 |  |
| fs/tmpfs.rs | `tmpfs_file_close` | 1510 | 19 |  |
| fs/tmpfs.rs | `is_tmpfs_file` | 1529 | 13 |  |
| fs/tmpfs.rs | `tmpfs_file_read_at` | 1542 | 25 |  |
| fs/tmpfs.rs | `tmpfs_get_file_ops` | 1567 | 164 |  |
| fs/tmpfs.rs | `mount_tmpfs` | 1731 | 12 |  |
| fs/tmpfs.rs | `mount_ramfs` | 1743 | 10 |  |
| fs/tmpfs.rs | `mount_devpts` | 1753 | 12 |  |
| fs/tmpfs.rs | `mount_securityfs` | 1765 | 11 |  |
| fs/tmpfs.rs | `mount_mqueue` | 1776 | 11 |  |
| fs/tmpfs.rs | `mount_configfs` | 1787 | 31 |  |
| fs/tmpfs.rs | `mount_register` | 1818 | 8 |  |
| fs/tracefs.rs | `next_ino` | 50 | 5 |  |
| fs/tracefs.rs | `tracefs_root` | 55 | 12 |  |
| fs/tracefs.rs | `create_dir` | 67 | 42 |  |
| fs/tracefs.rs | `create_file` | 109 | 15 |  |
| fs/tracefs.rs | `create_file_inner` | 124 | 68 |  |
| fs/tracefs.rs | `create_stream_file` | 192 | 24 |  |
| fs/tracefs.rs | `tracefs_mount_cb` | 216 | 13 |  |
| fs/tracefs.rs | `tracefs_kill_sb` | 229 | 11 |  |
| fs/tracefs.rs | `init_tracefs` | 240 | 35 |  |
| fs/tracefs.rs | `mount_at` | 275 | 15 |  |
| fs/tracefs.rs | `create_root_inode` | 290 | 45 |  |
| fs/tracefs.rs | `entry_of` | 335 | 5 |  |
| fs/tracefs.rs | `resolve_child` | 340 | 10 |  |
| fs/tracefs.rs | `tracefs_lookup` | 350 | 10 |  |
| fs/tracefs.rs | `tracefs_iget` | 360 | 30 |  |
| fs/tracefs.rs | `tracefs_getattr` | 390 | 51 |  |
| fs/tracefs.rs | `tracefs_setattr` | 441 | 48 |  |
| fs/tracefs.rs | `tracefs_readdir` | 489 | 27 |  |
| fs/tracefs.rs | `tracefs_open` | 516 | 30 |  |
| fs/tracefs.rs | `tracefs_get_file_ops` | 546 | 12 |  |
| fs/tracefs.rs | `tracefs_destroy_inode` | 558 | 46 |  |
| fs/tracefs.rs | `tracefs_file_read` | 604 | 35 |  |
| fs/tracefs.rs | `tracefs_file_write` | 639 | 25 |  |
| fs/tracefs.rs | `tracefs_file_lseek` | 664 | 21 |  |
| fs/tracefs.rs | `tracefs_file_close` | 685 | 22 |  |
| fs/tracefs.rs | `mount_tracefs` | 707 | 11 |  |
| fs/tracefs.rs | `mount_register` | 718 | 3 |  |
| fs/tty.rs | `default` | 132 | 26 | yes |
| fs/tty.rs | `canonical` | 158 | 15 |  |
| fs/tty.rs | `termios_to_kernel_bytes` | 173 | 12 |  |
| fs/tty.rs | `termios_from_kernel_bytes` | 185 | 18 |  |
| fs/tty.rs | `termios_to_user_bytes` | 203 | 15 |  |
| fs/tty.rs | `termios_from_user_bytes` | 218 | 18 |  |
| fs/tty.rs | `termios_to_termio_bytes` | 236 | 14 |  |
| fs/tty.rs | `termios_from_termio_bytes` | 250 | 27 |  |
| fs/tty.rs | `default` | 277 | 10 | yes |
| fs/tty.rs | `to_le_bytes` | 287 | 9 | yes |
| fs/tty.rs | `from_le_bytes` | 296 | 36 | yes |
| fs/tty.rs | `new` | 332 | 11 | yes |
| fs/tty.rs | `push` | 343 | 10 | yes |
| fs/tty.rs | `uncommitted_len` | 353 | 5 |  |
| fs/tty.rs | `erase_last` | 358 | 10 |  |
| fs/tty.rs | `kill_line` | 368 | 5 |  |
| fs/tty.rs | `commit` | 373 | 4 |  |
| fs/tty.rs | `set_eof` | 377 | 5 |  |
| fs/tty.rs | `readable_len` | 382 | 6 |  |
| fs/tty.rs | `read` | 388 | 31 | yes |
| fs/tty.rs | `isig_char` | 419 | 28 |  |
| fs/tty.rs | `new` | 447 | 15 | yes |
| fs/tty.rs | `set_output_ctx` | 462 | 7 |  |
| fs/tty.rs | `get_termios` | 469 | 5 |  |
| fs/tty.rs | `set_termios` | 474 | 5 |  |
| fs/tty.rs | `lflag` | 479 | 4 |  |
| fs/tty.rs | `echo_enabled` | 483 | 4 |  |
| fs/tty.rs | `get_winsize` | 487 | 5 |  |
| fs/tty.rs | `set_winsize` | 492 | 8 |  |
| fs/tty.rs | `send_sigwinch` | 500 | 12 |  |
| fs/tty.rs | `receive_byte` | 512 | 122 |  |
| fs/tty.rs | `send_isig` | 634 | 13 |  |
| fs/tty.rs | `fionread_count` | 647 | 6 |  |
| fs/tty.rs | `input_poll_ready` | 653 | 7 |  |
| fs/tty.rs | `read_input` | 660 | 75 |  |
| fs/tty.rs | `flush_input` | 735 | 11 |  |
| fs/tty.rs | `output_translates_nl` | 746 | 6 |  |
| fs/tty.rs | `write_output_raw` | 752 | 20 |  |
| fs/tty.rs | `console_output` | 772 | 17 |  |
| fs/tty.rs | `set_console_sink_quiet` | 789 | 11 |  |
| fs/tty.rs | `console` | 800 | 19 |  |
| fs/tty.rs | `file_is_tty` | 819 | 16 |  |
| fs/unsafe_edges.rs | `privdata_ref` | 35 | 18 |  |
| fs/unsafe_edges.rs | `file_privdata_ref` | 53 | 11 |  |
| fs/unsafe_edges.rs | `file_privdata_take` | 64 | 19 |  |
| fs/unsafe_edges.rs | `static_ref` | 83 | 13 |  |
| fs/unsafe_edges.rs | `static_ref_unchecked` | 96 | 8 |  |
| fs/unsafe_edges.rs | `file_privdata_arc_ref` | 104 | 15 |  |
| fs/unsafe_edges.rs | `file_privdata_arc_take` | 119 | 17 |  |
| fs/unsafe_edges.rs | `file_privdata_ref_mut` | 136 | 23 |  |
| fs/unsafe_edges.rs | `file_inode` | 159 | 11 |  |
| fs/unsafe_edges.rs | `file_inode_install` | 170 | 10 | yes |
| fs/unsafe_edges.rs | `file_privdata_take_arc` | 180 | 18 |  |
| fs/unsafe_edges.rs | `privdata_ref_leaked` | 198 | 9 |  |
| fs/unsafe_edges.rs | `file_inode_arc` | 207 | 12 |  |
| fs/unsafe_edges.rs | `file_ra_state_mut` | 219 | 10 |  |
| fs/unsafe_edges.rs | `as_deref_ref` | 229 | 3 |  |
| fs/unsafe_edges.rs | `as_deref_ref` | 232 | 10 |  |
| fs/unsafe_edges.rs | `privdata_take_box` | 242 | 14 |  |
| fs/unsafe_edges.rs | `privdata_drop_arc` | 256 | 10 |  |
| fs/unsafe_edges.rs | `kill_sb_reclaim_box` | 266 | 12 |  |
| fs/unsafe_edges.rs | `file_privdata_arc_clone` | 278 | 16 |  |
| fs/unsafe_edges.rs | `vec_zeroed_sized` | 294 | 13 |  |
| fs/unsafe_edges.rs | `kernel_slice` | 307 | 6 |  |
| fs/unsafe_edges.rs | `kernel_slice_mut` | 313 | 7 |  |
| fs/unsafe_edges.rs | `file_close` | 320 | 10 | yes |
| fs/unsafe_edges.rs | `file_privdata_arc_static` | 330 | 9 |  |
| fs/vfs.rs | `new` | 110 | 9 | yes |
| fs/vfs.rs | `with_inode` | 119 | 9 | yes |
| fs/vfs.rs | `is_valid` | 128 | 6 | yes |
| fs/vfs.rs | `default` | 134 | 58 | yes |
| fs/vfs.rs | `vfs_root_publish` | 192 | 20 | yes |
| fs/vfs.rs | `get_vfs_root` | 212 | 29 | yes |
| fs/vfs.rs | `follow_mount` | 241 | 18 | yes |
| fs/vfs.rs | `mount_flags_for_path` | 259 | 33 | yes |
| fs/vfs.rs | `path_on_readonly_mount` | 292 | 14 | yes |
| fs/vfs.rs | `file_on_readonly_mount` | 306 | 10 | yes |
| fs/vfs.rs | `dentry_on_readonly_mount` | 316 | 23 | yes |
| fs/vfs.rs | `check_not_readonly` | 339 | 20 | yes |
| fs/vfs.rs | `ns_may_reflag` | 359 | 12 | yes |
| fs/vfs.rs | `apply_mnt_flags` | 371 | 57 | yes |
| fs/vfs.rs | `vfs_remount_dentry` | 428 | 41 | yes |
| fs/vfs.rs | `vfs_remount` | 469 | 66 | yes |
| fs/vfs.rs | `shallow_dentry_walk` | 535 | 19 | yes |
| fs/vfs.rs | `vfs_mount` | 554 | 138 | yes |
| fs/vfs.rs | `dentry_in_mount_subtree` | 692 | 32 | yes |
| fs/vfs.rs | `mount_is_busy` | 724 | 61 | yes |
| fs/vfs.rs | `sync_dirty_filesystems` | 785 | 46 | yes |
| fs/vfs.rs | `vfs_umount` | 831 | 86 | yes |
| fs/vfs.rs | `vfs_umount_object` | 917 | 84 | yes |
| fs/vfs.rs | `dentry_relpath_below` | 1001 | 45 | yes |
| fs/vfs.rs | `mountpoint_chain` | 1046 | 15 | yes |
| fs/vfs.rs | `mount_index_insert` | 1061 | 18 | yes |
| fs/vfs.rs | `mount_index_remove` | 1079 | 16 | yes |
| fs/vfs.rs | `mount_index_rebuild` | 1095 | 32 | yes |
| fs/vfs.rs | `collect_subtree_mounts` | 1127 | 100 | yes |
| fs/vfs.rs | `vfs_bind_mount` | 1227 | 278 | yes |
| fs/vfs.rs | `bind_clone_attach` | 1505 | 228 | yes |
| fs/vfs.rs | `vfs_virtual_switch_root` | 1733 | 83 | yes |
| fs/vfs.rs | `vfs_move_mount` | 1816 | 170 | yes |
| fs/vfs.rs | `vfs_attach_mount_at` | 1986 | 76 | yes |
| fs/vfs.rs | `vfs_covering_mount` | 2062 | 32 | yes |
| fs/vfs.rs | `vfs_detach_mount` | 2094 | 34 | yes |
| fs/vfs.rs | `vfs_governing_mount_of` | 2128 | 49 | yes |
| fs/vfs.rs | `vfs_clone_submounts` | 2177 | 14 | yes |
| fs/vfs.rs | `vfs_collect_submounts` | 2191 | 9 | yes |
| fs/vfs.rs | `vfs_mount_registry_row` | 2200 | 25 | yes |
| fs/vfs.rs | `from_str` | 2225 | 16 | yes |
| fs/vfs.rs | `init` | 2241 | 27 | yes |
| fs/vfs.rs | `get_cwd` | 2268 | 18 | yes |
| fs/vfs.rs | `make_absolute` | 2286 | 20 | yes |
| fs/vfs.rs | `get_process_root` | 2306 | 18 | yes |
| fs/vfs.rs | `chrooted` | 2324 | 9 | yes |
| fs/vfs.rs | `process_rooted` | 2333 | 12 | yes |
| fs/vfs.rs | `path_is_normalized_fast` | 2345 | 44 | yes |
| fs/vfs.rs | `get_cwd_user` | 2389 | 41 | yes |
| fs/vfs.rs | `dentry_user_path` | 2430 | 18 | yes |
| fs/vfs.rs | `make_absolute_rooted` | 2448 | 14 | yes |
| fs/vfs.rs | `is_under_root` | 2462 | 13 | yes |
| fs/vfs.rs | `normalize_with_root` | 2475 | 38 | yes |
| fs/vfs.rs | `dentry_walk_no_symlink` | 2513 | 41 | yes |
| fs/vfs.rs | `sbx_want` | 2554 | 20 | yes |
| fs/vfs.rs | `sbx_noise` | 2574 | 4 | yes |
| fs/vfs.rs | `sbx_trace` | 2578 | 49 | yes |
| fs/vfs.rs | `sbx_dentry_state` | 2627 | 8 | yes |
| fs/vfs.rs | `path_lookup` | 2635 | 791 | yes |
| fs/vfs.rs | `follow_symlink` | 3426 | 17 | yes |
| fs/vfs.rs | `follow_symlink_hinted` | 3443 | 270 | yes |
| fs/vfs.rs | `path_parent_and_name` | 3713 | 33 | yes |
| fs/vfs.rs | `check_parent_write_permission` | 3746 | 22 | yes |
| fs/vfs.rs | `check_parent_exec_permission` | 3768 | 26 | yes |
| fs/vfs.rs | `check_sticky` | 3794 | 48 | yes |
| fs/vfs.rs | `dentry_parent_inode` | 3842 | 6 | yes |
| fs/vfs.rs | `notify_about` | 3848 | 13 | yes |
| fs/vfs.rs | `inode_nlink` | 3861 | 11 | yes |
| fs/vfs.rs | `lookup_parent_dir` | 3872 | 40 | yes |
| fs/vfs.rs | `vfs_dir_epoch` | 3912 | 6 | yes |
| fs/vfs.rs | `vfs_dir_epoch_bump` | 3918 | 10 | yes |
| fs/vfs.rs | `vfs_mkdir` | 3928 | 171 | yes |
| fs/vfs.rs | `vfs_mknod` | 4099 | 62 | yes |
| fs/vfs.rs | `vfs_symlink` | 4161 | 61 | yes |
| fs/vfs.rs | `vfs_rmdir` | 4222 | 163 | yes |
| fs/vfs.rs | `vfs_unlink` | 4385 | 141 | yes |
| fs/vfs.rs | `vfs_link` | 4526 | 74 | yes |
| fs/vfs.rs | `vfs_rename` | 4600 | 15 | yes |
| fs/vfs.rs | `vfs_rename_locked` | 4615 | 193 | yes |
| fs/vfs.rs | `is_ancestor_of` | 4808 | 49 | yes |
| fs/vfs.rs | `vfs_rename_exchange` | 4857 | 66 | yes |
| fs/vfs.rs | `vfs_chmod` | 4923 | 62 | yes |
| fs/vfs.rs | `vfs_chown` | 4985 | 104 | yes |
| fs/vfs.rs | `vfs_truncate` | 5089 | 66 | yes |
| fs/vfs.rs | `vfs_ftruncate` | 5155 | 66 | yes |
| fs/vfs.rs | `vfs_stat` | 5221 | 48 | yes |
| fs/vfs.rs | `in_group_or_capable_dir` | 5269 | 19 | yes |
| fs/vfs.rs | `mode_strip_sgid` | 5288 | 20 | yes |
| fs/vfs.rs | `prepare_create_mode` | 5308 | 17 | yes |
| fs/vfs.rs | `inherit_setgid_dir_gid` | 5325 | 48 | yes |
| fs/vfs.rs | `file_open_tmpfile` | 5373 | 54 | yes |
| fs/vfs.rs | `file_open` | 5427 | 418 | yes |
| fs/vfs.rs | `open_file_arc_nofd` | 5845 | 18 | yes |
| fs/vfs.rs | `open_file_arc_from_vpath` | 5863 | 31 | yes |
| fs/vfs.rs | `file_close` | 5894 | 14 | yes |
| fs/vfs.rs | `file_read` | 5908 | 27 | yes |
| fs/vfs.rs | `file_write` | 5935 | 19 | yes |
| fs/vfs.rs | `file_stat` | 5954 | 59 | yes |
| fs/vfs.rs | `stat_file_by_path` | 6013 | 5 | yes |
| fs/vfs.rs | `stat_file_by_path_with_flags` | 6018 | 101 | yes |
| fs/vfs.rs | `resolve_lock_region` | 6119 | 70 | yes |
| fs/vfs.rs | `file_fcntl` | 6189 | 580 | yes |
| fs/vfs.rs | `file_mkdir` | 6769 | 13 | yes |
| fs/vfs.rs | `file_rmdir` | 6782 | 13 | yes |
| fs/vfs.rs | `file_unlink` | 6795 | 14 | yes |
| fs/vfs.rs | `file_link` | 6809 | 16 | yes |
| fs/vfs.rs | `file_opendir` | 6825 | 67 | yes |
| fs/vfs.rs | `getdents_with_dots` | 6892 | 34 | yes |
| fs/vfs.rs | `file_getdents64` | 6926 | 194 | yes |
| fs/vfs.rs | `mem_file_content` | 7120 | 7 | yes |
| fs/vfs.rs | `mem_file_content_mut` | 7127 | 8 | yes |
| fs/vfs.rs | `mem_file_take_owned` | 7135 | 7 | yes |
| fs/vfs.rs | `mem_file_read` | 7142 | 19 | yes |
| fs/vfs.rs | `mem_file_stat` | 7161 | 28 | yes |
| fs/vfs.rs | `mem_file_lseek` | 7189 | 31 | yes |
| fs/vfs.rs | `mem_file_close` | 7220 | 15 | yes |
| fs/vfs.rs | `mem_file_poll` | 7235 | 36 | yes |
| fs/vfs.rs | `vfs_fallocate` | 7271 | 84 | yes |
| fs/vfs.rs | `vfs_utimensat` | 7355 | 9 | yes |
| fs/vfs.rs | `vfs_utimensat_ex` | 7364 | 84 | yes |
| fs/vfs.rs | `procfs_dir_readdir` | 7448 | 18 | yes |
| fs/vfs.rs | `open_procfs_dir` | 7466 | 16 | yes |
| fs/vfs.rs | `open_mem_file` | 7482 | 8 | yes |
| fs/vfs.rs | `open_mem_file_regen` | 7490 | 26 | yes |
| fs/vfs.rs | `pipe_fcntl` | 7516 | 12 | yes |
| fs/vt.rs | `is_vt_devno` | 123 | 6 |  |
| fs/vt.rs | `vt_opened` | 129 | 5 |  |
| fs/vt.rs | `vt_closed` | 134 | 5 |  |
| fs/vt.rs | `active_vt` | 139 | 5 |  |
| fs/vt.rs | `wake_switch_waiters` | 144 | 7 |  |
| fs/vt.rs | `wait_switch_round` | 151 | 24 |  |
| fs/vt.rs | `vtdev_read` | 175 | 4 |  |
| fs/vt.rs | `vtdev_write` | 179 | 4 |  |
| fs/vt.rs | `vtdev_poll` | 183 | 8 |  |
| fs/vt.rs | `vtdev_close` | 191 | 26 |  |
| fs/vt.rs | `vt_file_ioctl` | 217 | 223 |  |
| fs/xattr.rs | `check_name` | 68 | 28 |  |
| fs/xattr.rs | `may_set` | 96 | 65 |  |
| fs/xattr.rs | `set` | 161 | 67 | yes |
| fs/xattr.rs | `get` | 228 | 42 | yes |
| fs/xattr.rs | `list` | 270 | 33 |  |
| fs/xattr.rs | `remove` | 303 | 17 | yes |
