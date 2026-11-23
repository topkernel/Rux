# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# fs — hot 28128 lines (stay) / cold 19458 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| fs/bio.rs | `new` | 37 | 5 | yes |
| fs/bio.rs | `set` | 41 | 5 | yes |
| fs/bio.rs | `clear` | 45 | 5 | yes |
| fs/bio.rs | `test` | 49 | 5 | yes |
| fs/bio.rs | `is_uptodate` | 53 | 5 | yes |
| fs/bio.rs | `is_dirty` | 57 | 5 | yes |
| fs/bio.rs | `is_locked` | 61 | 5 | yes |
| fs/bio.rs | `is_mapped` | 65 | 34 | yes |
| fs/bio.rs | `new` | 98 | 20 | yes |
| fs/bio.rs | `new_uninit` | 117 | 19 | yes |
| fs/bio.rs | `set_device` | 135 | 6 | yes |
| fs/bio.rs | `get_state` | 140 | 7 | yes |
| fs/bio.rs | `set_state_bit` | 146 | 18 | yes |
| fs/bio.rs | `clear_state_bit` | 163 | 7 | yes |
| fs/bio.rs | `is_dirty` | 169 | 7 | yes |
| fs/bio.rs | `get` | 175 | 6 | yes |
| fs/bio.rs | `put` | 180 | 6 | yes |
| fs/bio.rs | `count` | 185 | 6 | yes |
| fs/bio.rs | `read` | 190 | 12 | yes |
| fs/bio.rs | `write` | 201 | 13 | yes |
| fs/bio.rs | `sync` | 213 | 45 | yes |
| fs/bio.rs | `new` | 257 | 20 | yes |
| fs/bio.rs | `bh` | 276 | 17 | yes |
| fs/bio.rs | `bh_mut` | 292 | 19 | yes |
| fs/bio.rs | `entry_ref` | 310 | 8 | yes |
| fs/bio.rs | `drop` | 317 | 73 | yes |
| fs/bio.rs | `cache_block_size` | 389 | 5 | yes |
| fs/bio.rs | `new` | 393 | 23 | yes |
| fs/bio.rs | `lru_lock_under_bucket` | 415 | 11 | yes |
| fs/bio.rs | `lru_lock_under_bucket_irqsave` | 425 | 9 | yes |
| fs/bio.rs | `hash_index` | 433 | 11 | yes |
| fs/bio.rs | `move_to_lru_head` | 443 | 46 | yes |
| fs/bio.rs | `push_lru_head` | 488 | 26 | yes |
| fs/bio.rs | `remove_from_lru` | 513 | 26 | yes |
| fs/bio.rs | `evict_one` | 538 | 182 | yes |
| fs/bio.rs | `drop_block` | 719 | 62 | yes |
| fs/bio.rs | `get` | 780 | 240 | yes |
| fs/bio.rs | `get_zero` | 1019 | 13 | yes |
| fs/bio.rs | `get_overwrite` | 1031 | 5 | yes |
| fs/bio.rs | `get_zero_impl` | 1035 | 124 | yes |
| fs/bio.rs | `put` | 1158 | 13 | yes |
| fs/bio.rs | `sync_all` | 1170 | 64 | yes |
| fs/bio.rs | `sync_all_excluding` | 1233 | 47 | yes |
| fs/bio.rs | `invalidate` | 1279 | 40 | yes |
| fs/bio.rs | `invalidate_device` | 1318 | 55 | yes |
| fs/bio.rs | `drop` | 1372 | 17 | yes |
| fs/bio.rs | `get_block_cache` | 1388 | 51 | yes |
| fs/bio.rs | `bh_pinned` | 1438 | 8 | yes |
| fs/bio.rs | `bh_data` | 1445 | 6 | yes |
| fs/bio.rs | `bh_data_mut` | 1450 | 11 | yes |
| fs/bio.rs | `bh_blocknr` | 1460 | 6 | yes |
| fs/bio.rs | `bh_set_state_bit` | 1465 | 6 | yes |
| fs/bio.rs | `bh_clear_state_bit` | 1470 | 6 | yes |
| fs/bio.rs | `bh_is_dirty` | 1475 | 5 | yes |
| fs/bio.rs | `bread` | 1479 | 9 | yes |
| fs/bio.rs | `getblk_zero` | 1487 | 9 | yes |
| fs/bio.rs | `getblk_overwrite` | 1495 | 8 | yes |
| fs/bio.rs | `drop_freed_block` | 1502 | 12 | yes |
| fs/bio.rs | `drop_freed_block_forgotten` | 1513 | 23 | yes |
| fs/bio.rs | `invalidate_device` | 1535 | 27 | yes |
| fs/bio.rs | `wait_buffer_io_done` | 1561 | 39 | yes |
| fs/bio.rs | `bread_async` | 1599 | 262 | yes |
| fs/bio.rs | `bread_wait` | 1860 | 38 | yes |
| fs/bio.rs | `brelse` | 1897 | 16 | yes |
| fs/bio.rs | `block_cached` | 1912 | 50 | yes |
| fs/bio.rs | `block_dirty_cached` | 1961 | 46 | yes |
| fs/bio.rs | `block_absent_durable` | 2006 | 42 | yes |
| fs/bio.rs | `bread_async_multi` | 2047 | 43 | yes |
| fs/bio.rs | `bfree_multi` | 2089 | 137 | yes |
| fs/bio.rs | `as_ptr` | 2225 | 9 | yes |
| fs/bio.rs | `as_slice` | 2233 | 20 | yes |
| fs/bio.rs | `len` | 2252 | 10 | yes |
| fs/bio.rs | `new` | 2261 | 13 | yes |
| fs/bio.rs | `in_flight` | 2273 | 9 | yes |
| fs/bio.rs | `status` | 2281 | 12 | yes |
| fs/bio.rs | `mode_probe` | 2292 | 21 | yes |
| fs/bio.rs | `submit` | 2312 | 57 | yes |
| fs/bio.rs | `submit_ref` | 2368 | 51 | yes |
| fs/bio.rs | `flush` | 2418 | 14 | yes |
| fs/bio.rs | `flush_immediate` | 2431 | 29 | yes |
| fs/bio.rs | `wait_status_apply` | 2459 | 66 | yes |
| fs/bio.rs | `flush_deferred` | 2524 | 188 | yes |
| fs/bio.rs | `retire_window` | 2711 | 16 | yes |
| fs/bio.rs | `default` | 2726 | 11 | yes |
| fs/bio.rs | `drop` | 2736 | 20 | yes |
| fs/bio.rs | `sync_dirty_blocks_batched` | 2755 | 6 | yes |
| fs/bio.rs | `drain` | 2760 | 100 | yes |
| fs/bio.rs | `sync_dirty_buffer` | 2859 | 35 | yes |
| fs/bio.rs | `sync_buffers` | 2893 | 13 | yes |
| fs/bio.rs | `sync_device_buffers_excluding` | 2905 | 11 | yes |
| fs/bio.rs | `init` | 2915 | 120 | yes |
| fs/bio.rs | `new` | 3034 | 7 | yes |
| fs/bio.rs | `index_record` | 3040 | 9 | yes |
| fs/bio.rs | `push` | 3048 | 6 | yes |
| fs/bio.rs | `post_capture` | 3053 | 13 | yes |
| fs/bio.rs | `take_list` | 3065 | 6 | yes |
| fs/bio.rs | `set_list` | 3070 | 8 | yes |
| fs/bio.rs | `is_empty` | 3077 | 4 | yes |
| fs/bio.rs | `len` | 3080 | 4 | yes |
| fs/bio.rs | `iter` | 3083 | 6 | yes |
| fs/bio.rs | `constrained` | 3088 | 32 | yes |
| fs/bio.rs | `publication_batch_drain` | 3119 | 20 | yes |
| fs/bio.rs | `publication_cookie` | 3138 | 8 | yes |
| fs/bio.rs | `dev_key` | 3145 | 7 | yes |
| fs/bio.rs | `dev_key_of` | 3151 | 8 | yes |
| fs/bio.rs | `publication_defer` | 3158 | 42 | yes |
| fs/bio.rs | `publication_defer_entry_only` | 3199 | 39 | yes |
| fs/bio.rs | `publication_capture_note_dirty` | 3237 | 59 | yes |
| fs/bio.rs | `publication_constrained` | 3295 | 73 | yes |
| fs/bio.rs | `new` | 3367 | 5 | yes |
| fs/bio.rs | `note_queue_entries` | 3371 | 14 | yes |
| fs/bio.rs | `push_pre` | 3384 | 15 | yes |
| fs/bio.rs | `push_entry` | 3398 | 7 | yes |
| fs/bio.rs | `push_post` | 3404 | 40 | yes |
| fs/bio.rs | `plan_add_constraints` | 3443 | 62 | yes |
| fs/bio.rs | `scheduled` | 3504 | 5 | yes |
| fs/bio.rs | `scheduled_pre_of` | 3508 | 11 | yes |
| fs/bio.rs | `flush_publications_scoped` | 3518 | 101 | yes |
| fs/bio.rs | `flush_publication_constraints` | 3618 | 72 | yes |
| fs/bio.rs | `flush_publications` | 3689 | 10 | yes |
| fs/bio.rs | `flush_publications_bounded` | 3698 | 53 | yes |
| fs/bio.rs | `sync_block_ordered` | 3750 | 19 | yes |
| fs/bio.rs | `peek_buffer` | 3768 | 30 | yes |
| fs/bio.rs | `with_block_mut` | 3797 | 30 | yes |
| fs/bio.rs | `with_block_deferred_mut` | 3826 | 24 | yes |
| fs/bio.rs | `with_block` | 3849 | 24 | yes |
| fs/bio.rs | `with_block_ro` | 3872 | 12 | yes |
| fs/buffer.rs | `new` | 38 | 15 | yes |
| fs/buffer.rs | `from_bytes` | 52 | 17 | yes |
| fs/buffer.rs | `read` | 68 | 12 | yes |
| fs/buffer.rs | `write` | 79 | 12 | yes |
| fs/buffer.rs | `get` | 90 | 6 | yes |
| fs/buffer.rs | `put` | 95 | 16 | yes |
| fs/buffer.rs | `new` | 110 | 9 | yes |
| fs/buffer.rs | `get_size` | 118 | 6 | yes |
| fs/buffer.rs | `set_size` | 123 | 8 | yes |
| fs/buffer.rs | `read` | 130 | 52 | yes |
| fs/buffer.rs | `write` | 181 | 45 | yes |
| fs/buffer.rs | `load_from_bytes` | 225 | 29 | yes |
| fs/buffer.rs | `truncate` | 253 | 14 | yes |
| fs/buffer.rs | `default` | 266 | 29 | yes |
| fs/buffer.rs | `new` | 294 | 26 | yes |
| fs/buffer.rs | `base_ptr` | 319 | 15 | yes |
| fs/buffer.rs | `drop` | 333 | 20 | yes |
| fs/buffer.rs | `new_chunk` | 352 | 11 | yes |
| fs/buffer.rs | `new` | 362 | 9 | yes |
| fs/buffer.rs | `from_bytes` | 370 | 8 | yes |
| fs/buffer.rs | `read` | 377 | 28 | yes |
| fs/buffer.rs | `write` | 404 | 43 | yes |
| fs/buffer.rs | `resize` | 446 | 36 | yes |
| fs/buffer.rs | `len` | 481 | 6 | yes |
| fs/buffer.rs | `is_empty` | 486 | 7 | yes |
| fs/buffer.rs | `default` | 492 | 5 | yes |
| fs/cgroup.rs | `control_file_by_name` | 122 | 27 |  |
| fs/cgroup.rs | `ino_of_dir` | 148 | 5 |  |
| fs/cgroup.rs | `ino_of_file` | 152 | 9 |  |
| fs/cgroup.rs | `generate_procs` | 160 | 9 |  |
| fs/cgroup.rs | `generate_controllers` | 168 | 12 |  |
| fs/cgroup.rs | `generate_subtree_control` | 179 | 8 |  |
| fs/cgroup.rs | `generate_events` | 186 | 5 |  |
| fs/cgroup.rs | `generate_type` | 190 | 5 |  |
| fs/cgroup.rs | `generate_cpu_max` | 194 | 15 |  |
| fs/cgroup.rs | `generate_cpu_stat` | 208 | 12 |  |
| fs/cgroup.rs | `generate_memory_max` | 219 | 14 |  |
| fs/cgroup.rs | `generate_memory_current` | 232 | 8 |  |
| fs/cgroup.rs | `generate_memory_stat` | 239 | 8 |  |
| fs/char_dev.rs | `new` | 34 | 6 | yes |
| fs/char_dev.rs | `read` | 39 | 9 | yes |
| fs/char_dev.rs | `write` | 47 | 9 | yes |
| fs/char_dev.rs | `uart_read` | 55 | 43 |  |
| fs/char_dev.rs | `uart_write_kernel` | 97 | 16 | yes |
| fs/char_dev.rs | `uart_write` | 112 | 25 |  |
| fs/char_dev.rs | `uart_file_poll` | 136 | 33 |  |
| fs/char_dev.rs | `char_dev_file_stat` | 168 | 26 |  |
| fs/char_dev.rs | `uart_file_read` | 193 | 18 |  |
| fs/char_dev.rs | `uart_file_write` | 210 | 11 |  |
| fs/char_dev.rs | `char_dev_stat` | 220 | 33 |  |
| fs/debugfs.rs | `next_ino` | 91 | 6 |  |
| fs/debugfs.rs | `debugfs_root` | 96 | 13 |  |
| fs/debugfs.rs | `create_dir` | 108 | 43 |  |
| fs/debugfs.rs | `create_file` | 150 | 79 | yes |
| fs/debugfs.rs | `debugfs_mount_cb` | 228 | 13 |  |
| fs/debugfs.rs | `debugfs_kill_sb` | 240 | 12 |  |
| fs/debugfs.rs | `init_debugfs` | 251 | 34 |  |
| fs/debugfs.rs | `is_mounted` | 284 | 7 | yes |
| fs/debugfs.rs | `mount_debugfs` | 290 | 17 |  |
| fs/debugfs.rs | `create_root_inode` | 306 | 32 | yes |
| fs/debugfs.rs | `entry_of` | 337 | 6 |  |
| fs/debugfs.rs | `resolve_child` | 342 | 11 |  |
| fs/debugfs.rs | `debugfs_lookup` | 352 | 11 |  |
| fs/debugfs.rs | `debugfs_iget` | 362 | 22 |  |
| fs/debugfs.rs | `debugfs_getattr` | 383 | 34 |  |
| fs/debugfs.rs | `debugfs_setattr` | 416 | 10 |  |
| fs/debugfs.rs | `debugfs_readdir` | 425 | 26 |  |
| fs/debugfs.rs | `debugfs_open` | 450 | 20 |  |
| fs/debugfs.rs | `debugfs_get_file_ops` | 469 | 13 |  |
| fs/debugfs.rs | `debugfs_destroy_inode` | 481 | 44 |  |
| fs/debugfs.rs | `debugfs_file_read` | 524 | 21 |  |
| fs/debugfs.rs | `debugfs_file_write` | 544 | 26 |  |
| fs/debugfs.rs | `debugfs_file_lseek` | 569 | 22 |  |
| fs/debugfs.rs | `debugfs_file_close` | 590 | 24 |  |
| fs/debugfs.rs | `mount_debugfs_instance` | 613 | 15 |  |
| fs/debugfs.rs | `mount_register` | 627 | 4 | yes |
| fs/dentry.rs | `new` | 40 | 5 | yes |
| fs/dentry.rs | `is_hashed` | 44 | 5 | yes |
| fs/dentry.rs | `is_unhashed` | 48 | 5 | yes |
| fs/dentry.rs | `bits` | 52 | 40 | yes |
| fs/dentry.rs | `new` | 91 | 105 | yes |
| fs/dentry.rs | `acquire` | 195 | 18 | yes |
| fs/dentry.rs | `drop` | 212 | 13 | yes |
| fs/dentry.rs | `retire_dentry_arc` | 224 | 6 | yes |
| fs/dentry.rs | `retire_inode_arc` | 229 | 24 | yes |
| fs/dentry.rs | `dentry_drain_if_quiescent` | 252 | 71 | yes |
| fs/dentry.rs | `dentry_arc_from_raw` | 322 | 10 | yes |
| fs/dentry.rs | `inode_arc_from_raw` | 331 | 8 | yes |
| fs/dentry.rs | `child_name_hash` | 338 | 84 | yes |
| fs/dentry.rs | `empty` | 421 | 19 | yes |
| fs/dentry.rs | `memo_slot_read` | 439 | 122 | yes |
| fs/dentry.rs | `new` | 560 | 28 | yes |
| fs/dentry.rs | `neg_gen_load` | 587 | 21 | yes |
| fs/dentry.rs | `memo_negative` | 607 | 51 | yes |
| fs/dentry.rs | `set_parent` | 657 | 6 | yes |
| fs/dentry.rs | `set_inode` | 662 | 27 | yes |
| fs/dentry.rs | `clear_inode` | 688 | 17 | yes |
| fs/dentry.rs | `inode_ptr_walk` | 704 | 7 | yes |
| fs/dentry.rs | `get_inode_fast` | 710 | 16 | yes |
| fs/dentry.rs | `get_inode_gen` | 725 | 7 | yes |
| fs/dentry.rs | `set_inode_gen` | 731 | 6 | yes |
| fs/dentry.rs | `get_inode` | 736 | 6 | yes |
| fs/dentry.rs | `get_name` | 741 | 6 | yes |
| fs/dentry.rs | `build_path` | 746 | 23 | yes |
| fs/dentry.rs | `build_path_rooted` | 768 | 91 | yes |
| fs/dentry.rs | `lookup_child` | 858 | 22 | yes |
| fs/dentry.rs | `lookup_child_walk` | 879 | 55 | yes |
| fs/dentry.rs | `lookup_child_fast` | 933 | 19 | yes |
| fs/dentry.rs | `refill_memo_locked` | 951 | 74 | yes |
| fs/dentry.rs | `big_memo_ensure_locked` | 1024 | 50 | yes |
| fs/dentry.rs | `add_child` | 1073 | 46 | yes |
| fs/dentry.rs | `remove_child` | 1118 | 32 | yes |
| fs/dentry.rs | `clear_children` | 1149 | 21 | yes |
| fs/dentry.rs | `rehome_children_into` | 1169 | 28 | yes |
| fs/dentry.rs | `clear_mount` | 1196 | 12 | yes |
| fs/dentry.rs | `take_mount` | 1207 | 8 | yes |
| fs/dentry.rs | `set_mount` | 1214 | 7 | yes |
| fs/dentry.rs | `get_mount` | 1220 | 10 | yes |
| fs/dentry.rs | `has_mount` | 1229 | 7 | yes |
| fs/dentry.rs | `set_mount_flag` | 1235 | 8 | yes |
| fs/dentry.rs | `set_mountpoint_back` | 1242 | 8 | yes |
| fs/dentry.rs | `get_mountpoint_back` | 1249 | 6 | yes |
| fs/dentry.rs | `clear_mountpoint_back` | 1254 | 7 | yes |
| fs/dentry.rs | `mountpoint_dentry` | 1260 | 6 | yes |
| fs/dentry.rs | `set_hashed` | 1265 | 8 | yes |
| fs/dentry.rs | `set_unhashed` | 1272 | 8 | yes |
| fs/dentry.rs | `inc_ref` | 1279 | 6 | yes |
| fs/dentry.rs | `dec_ref` | 1284 | 6 | yes |
| fs/dentry.rs | `get_ref` | 1289 | 6 | yes |
| fs/dentry.rs | `set_negative` | 1294 | 6 | yes |
| fs/dentry.rs | `is_negative` | 1299 | 6 | yes |
| fs/dentry.rs | `clear_negative` | 1304 | 7 | yes |
| fs/dentry.rs | `make_root_dentry` | 1310 | 27 | yes |
| fs/dentry.rs | `new` | 1336 | 9 | yes |
| fs/dentry.rs | `record_hit` | 1344 | 5 | yes |
| fs/dentry.rs | `record_miss` | 1348 | 5 | yes |
| fs/dentry.rs | `record_eviction` | 1352 | 5 | yes |
| fs/dentry.rs | `get_hit_rate` | 1356 | 26 | yes |
| fs/dentry.rs | `clone` | 1381 | 30 | yes |
| fs/dentry.rs | `dcache_init` | 1410 | 28 | yes |
| fs/dentry.rs | `dentry_hash` | 1437 | 19 | yes |
| fs/dentry.rs | `dcache_lookup` | 1455 | 40 | yes |
| fs/dentry.rs | `dcache_add` | 1494 | 48 | yes |
| fs/dentry.rs | `dcache_evict_lru` | 1541 | 38 | yes |
| fs/dentry.rs | `dcache_remove` | 1578 | 28 | yes |
| fs/dentry.rs | `dcache_stats` | 1605 | 12 | yes |
| fs/dentry.rs | `dcache_stats_detailed` | 1616 | 18 | yes |
| fs/dentry.rs | `dcache_flush` | 1633 | 21 | yes |
| fs/dev_t.rs | `new` | 29 | 6 | yes |
| fs/dev_t.rs | `from_u64` | 34 | 9 |  |
| fs/dev_t.rs | `to_u64` | 42 | 10 |  |
| fs/dev_t.rs | `to_user_dev` | 51 | 12 | yes |
| fs/dev_t.rs | `from_user_dev` | 62 | 10 | yes |
| fs/dev_t.rs | `default` | 71 | 79 | yes |
| fs/devfs/mod.rs | `new_dir` | 126 | 17 |  |
| fs/devfs/mod.rs | `new_file` | 142 | 21 |  |
| fs/devfs/mod.rs | `new_symlink` | 162 | 17 |  |
| fs/devfs/mod.rs | `new_char_device` | 178 | 17 |  |
| fs/devfs/mod.rs | `new_char_device_with_mode` | 194 | 19 |  |
| fs/devfs/mod.rs | `new_block_device` | 212 | 17 |  |
| fs/devfs/mod.rs | `is_dir` | 228 | 6 | yes |
| fs/devfs/mod.rs | `is_char_device` | 233 | 6 | yes |
| fs/devfs/mod.rs | `is_block_device` | 238 | 6 | yes |
| fs/devfs/mod.rs | `is_regular_file` | 243 | 6 | yes |
| fs/devfs/mod.rs | `is_symlink` | 248 | 18 | yes |
| fs/devfs/mod.rs | `nulldev_read` | 265 | 5 |  |
| fs/devfs/mod.rs | `nulldev_write` | 269 | 8 |  |
| fs/devfs/mod.rs | `nulldev_poll` | 276 | 23 |  |
| fs/devfs/mod.rs | `condev_read` | 298 | 10 |  |
| fs/devfs/mod.rs | `condev_write` | 307 | 44 |  |
| fs/devfs/mod.rs | `condev_poll` | 350 | 20 |  |
| fs/devfs/mod.rs | `zerodev_read` | 369 | 6 |  |
| fs/devfs/mod.rs | `zerodev_write` | 374 | 6 |  |
| fs/devfs/mod.rs | `zerodev_poll` | 379 | 13 |  |
| fs/devfs/mod.rs | `fulldev_write` | 391 | 16 |  |
| fs/devfs/mod.rs | `rng_fill` | 406 | 18 |  |
| fs/devfs/mod.rs | `randdev_read` | 423 | 6 |  |
| fs/devfs/mod.rs | `randdev_poll` | 428 | 33 |  |
| fs/devfs/mod.rs | `root_disk_present` | 460 | 19 |  |
| fs/devfs/mod.rs | `devtmpfs_populate` | 478 | 230 |  |
| fs/devfs/mod.rs | `mknod_user` | 707 | 16 | yes |
| fs/devfs/mod.rs | `evict_dev_dentry` | 722 | 24 |  |
| fs/devfs/mod.rs | `devtmpfs_register_device` | 745 | 33 |  |
| fs/devfs/mod.rs | `devtmpfs_unregister_device` | 777 | 23 |  |
| fs/devfs/mod.rs | `init` | 799 | 149 | yes |
| fs/devfs/mod.rs | `mknod` | 947 | 72 | yes |
| fs/devfs/mod.rs | `remove_node` | 1018 | 65 |  |
| fs/devfs/mod.rs | `mkdir` | 1082 | 51 |  |
| fs/devfs/mod.rs | `lookup` | 1132 | 46 | yes |
| fs/devfs/mod.rs | `is_mounted` | 1177 | 6 | yes |
| fs/devfs/mod.rs | `get_root_entry` | 1182 | 15 |  |
| fs/devfs/mod.rs | `list_dir` | 1196 | 49 | yes |
| fs/devfs/mod.rs | `parse_dev_path` | 1244 | 21 | yes |
| fs/devfs/mod.rs | `devfs_lookup` | 1264 | 22 |  |
| fs/devfs/mod.rs | `devfs_iget` | 1285 | 58 |  |
| fs/devfs/mod.rs | `devfs_getattr` | 1342 | 48 |  |
| fs/devfs/mod.rs | `devfs_ino_hash` | 1389 | 27 |  |
| fs/devfs/mod.rs | `devfs_open` | 1415 | 78 |  |
| fs/devfs/mod.rs | `devfs_get_file_ops` | 1492 | 36 |  |
| fs/devfs/mod.rs | `devfs_readdir` | 1527 | 38 |  |
| fs/devfs/mod.rs | `devfs_destroy_inode` | 1564 | 17 |  |
| fs/devfs/mod.rs | `devfs_rel_path` | 1580 | 2 |  |
| fs/devfs/mod.rs | `walk` | 1581 | 29 | yes |
| fs/devfs/mod.rs | `devfs_mkdir` | 1609 | 38 |  |
| fs/devfs/mod.rs | `devfs_create` | 1646 | 35 |  |
| fs/devfs/mod.rs | `devfs_symlink` | 1680 | 37 |  |
| fs/devfs/mod.rs | `devfs_readlink` | 1716 | 22 |  |
| fs/devfs/mod.rs | `devfs_link` | 1737 | 57 |  |
| fs/devfs/mod.rs | `devfs_rename` | 1793 | 98 |  |
| fs/devfs/mod.rs | `new` | 1890 | 8 | yes |
| fs/devfs/mod.rs | `copy_in` | 1897 | 10 | yes |
| fs/devfs/mod.rs | `copy_out` | 1906 | 14 | yes |
| fs/devfs/mod.rs | `zero` | 1919 | 7 | yes |
| fs/devfs/mod.rs | `shared_grow_locked` | 1925 | 76 |  |
| fs/devfs/mod.rs | `devfs_reg_current_size` | 2000 | 19 |  |
| fs/devfs/mod.rs | `shared_fault_file_page` | 2018 | 52 |  |
| fs/devfs/mod.rs | `shared_entry_of` | 2069 | 21 |  |
| fs/devfs/mod.rs | `shared_read_off` | 2089 | 26 |  |
| fs/devfs/mod.rs | `shared_write_off` | 2114 | 35 |  |
| fs/devfs/mod.rs | `devfs_file_read` | 2148 | 32 |  |
| fs/devfs/mod.rs | `devfs_file_write` | 2179 | 37 |  |
| fs/devfs/mod.rs | `devfs_file_lseek` | 2215 | 34 |  |
| fs/devfs/mod.rs | `devfs_file_read_at` | 2248 | 43 |  |
| fs/devfs/mod.rs | `devfs_unlink` | 2290 | 33 |  |
| fs/devfs/mod.rs | `devfs_rmdir` | 2322 | 51 |  |
| fs/devfs/mod.rs | `devfs_setattr` | 2372 | 108 |  |
| fs/devfs/mod.rs | `create_root_inode` | 2479 | 16 | yes |
| fs/devfs/mod.rs | `mount_devtmpfs` | 2494 | 13 |  |
| fs/devfs/mod.rs | `mount_register` | 2506 | 4 | yes |
| fs/devfs/registry.rs | `new` | 27 | 19 | yes |
| fs/devfs/registry.rs | `register_char_device` | 45 | 14 | yes |
| fs/devfs/registry.rs | `unregister_char_device` | 58 | 13 |  |
| fs/devfs/registry.rs | `get_char_device_ops` | 70 | 7 |  |
| fs/devfs/registry.rs | `is_device_registered` | 76 | 7 |  |
| fs/devfs/registry.rs | `device_count` | 82 | 5 |  |
| fs/elf.rs | `from_bytes` | 348 | 48 | yes |
| fs/elf.rs | `is_executable` | 395 | 7 | yes |
| fs/elf.rs | `is_dynamic` | 401 | 6 |  |
| fs/elf.rs | `check_machine` | 406 | 17 |  |
| fs/elf.rs | `get_program_headers` | 422 | 12 | yes |
| fs/elf.rs | `get_program_header` | 433 | 21 |  |
| fs/elf.rs | `is_load` | 453 | 6 |  |
| fs/elf.rs | `is_readable` | 458 | 6 | yes |
| fs/elf.rs | `is_writable` | 463 | 6 | yes |
| fs/elf.rs | `is_executable` | 468 | 25 | yes |
| fs/elf.rs | `validate` | 492 | 26 | yes |
| fs/elf.rs | `get_entry` | 517 | 8 | yes |
| fs/elf.rs | `get_program_headers` | 524 | 16 | yes |
| fs/elf.rs | `load` | 539 | 86 | yes |
| fs/elf.rs | `load_segment` | 624 | 35 |  |
| fs/elf.rs | `get_interpreter` | 658 | 51 | yes |
| fs/event_notify.rs | `list_first` | 77 | 8 | yes |
| fs/event_notify.rs | `list_next` | 84 | 7 | yes |
| fs/event_notify.rs | `list_link_tail` | 90 | 9 | yes |
| fs/event_notify.rs | `list_unlink` | 98 | 8 | yes |
| fs/event_notify.rs | `boxed_mut` | 105 | 7 | yes |
| fs/event_notify.rs | `boxed_drop` | 111 | 8 | yes |
| fs/event_notify.rs | `list_entry_mut` | 118 | 8 | yes |
| fs/event_notify.rs | `list_entry_pub` | 125 | 6 | yes |
| fs/event_notify.rs | `list_entry` | 130 | 13 | yes |
| fs/event_notify.rs | `notify_pin_acquire` | 142 | 3 |  |
| fs/event_notify.rs | `notify_pin_release` | 144 | 3 |  |
| fs/event_notify.rs | `rearm_edge` | 146 | 3 |  |
| fs/event_notify.rs | `wake_waiters` | 148 | 5 |  |
| fs/event_notify.rs | `boxed_ref` | 152 | 52 | yes |
| fs/event_notify.rs | `locked_head` | 203 | 5 |  |
| fs/event_notify.rs | `init_once` | 207 | 37 | yes |
| fs/event_notify.rs | `epoll_shard_index` | 243 | 8 |  |
| fs/event_notify.rs | `epoll_wake_shard` | 250 | 22 | yes |
| fs/event_notify.rs | `epoll_notify_file` | 271 | 86 | yes |
| fs/event_notify.rs | `no_wake` | 356 | 76 |  |
| fs/event_notify.rs | `epoll_wake_register` | 431 | 54 | yes |
| fs/event_notify.rs | `epoll_wake_unregister_all` | 484 | 53 | yes |
| fs/event_notify.rs | `epoll_wake_unregister` | 536 | 54 | yes |
| fs/event_notify.rs | `signalfd_notify_task` | 589 | 28 |  |
| fs/event_notify.rs | `signalfd_register` | 616 | 11 | yes |
| fs/event_notify.rs | `signalfd_unregister` | 626 | 11 | yes |
| fs/ext4_edges.rs | `typed_ref` | 23 | 11 | yes |
| fs/ext4_edges.rs | `typed_ref_mut` | 33 | 14 | yes |
| fs/ext4_edges.rs | `typed_slice` | 46 | 21 | yes |
| fs/ext4_edges.rs | `typed_slice_mut` | 66 | 24 | yes |
| fs/ext4_edges.rs | `u32_array_ref` | 89 | 12 | yes |
| fs/ext4_edges.rs | `u32_array_ref_mut` | 100 | 18 | yes |
| fs/ext4_edges.rs | `zeroed_on_disk` | 117 | 6 | yes |
| fs/ext4_hooks.rs | `ext4_register_hooks` | 164 | 6 | yes |
| fs/ext4_hooks.rs | `hooks` | 169 | 7 |  |
| fs/ext4_hooks.rs | `init` | 175 | 4 | yes |
| fs/ext4_hooks.rs | `mount_root` | 178 | 4 |  |
| fs/ext4_hooks.rs | `root_inode` | 181 | 4 |  |
| fs/ext4_hooks.rs | `mounted` | 184 | 4 |  |
| fs/ext4_hooks.rs | `unmount` | 187 | 4 |  |
| fs/ext4_hooks.rs | `mount_instance` | 190 | 4 |  |
| fs/ext4_hooks.rs | `loop_invalidate` | 193 | 4 |  |
| fs/ext4_hooks.rs | `boot_read_file` | 196 | 4 |  |
| fs/ext4_hooks.rs | `fsctx_mount_instance` | 199 | 4 |  |
| fs/ext4_hooks.rs | `mount_rows_register` | 202 | 4 |  |
| fs/ext4_hooks.rs | `sync_flushed_data` | 205 | 4 | yes |
| fs/ext4_hooks.rs | `sync_inode_tables` | 208 | 4 | yes |
| fs/ext4_hooks.rs | `shutdown_sync` | 211 | 4 |  |
| fs/ext4_hooks.rs | `remount_ro_latch` | 214 | 4 | yes |
| fs/ext4_hooks.rs | `fallocate` | 217 | 4 | yes |
| fs/ext4_hooks.rs | `default_inode_ops` | 220 | 4 |  |
| fs/ext4_hooks.rs | `is_ext4_inode_ops` | 223 | 4 | yes |
| fs/ext4_hooks.rs | `is_ext4_lookup` | 226 | 4 | yes |
| fs/ext4_hooks.rs | `is_ext4_file` | 229 | 4 | yes |
| fs/ext4_hooks.rs | `writeback_flush` | 232 | 4 | yes |
| fs/ext4_hooks.rs | `kmsg_resolve` | 235 | 4 |  |
| fs/ext4_hooks.rs | `kmsg_write_at` | 238 | 4 |  |
| fs/ext4_hooks.rs | `journal_dirty` | 241 | 4 | yes |
| fs/ext4_hooks.rs | `journal_forget` | 244 | 4 | yes |
| fs/ext4_hooks.rs | `exit_journal_unwind` | 247 | 4 |  |
| fs/ext4_hooks.rs | `exthunt_phys_watched` | 250 | 4 | yes |
| fs/ext4_hooks.rs | `exthunt_evict_unhash` | 253 | 4 | yes |
| fs/ext4_hooks.rs | `exthunt_evict_done` | 256 | 4 | yes |
| fs/ext4_hooks.rs | `exthunt_fab` | 259 | 4 | yes |
| fs/ext4_hooks.rs | `exthunt_bc_blind` | 262 | 4 | yes |
| fs/ext4_hooks.rs | `exthunt_rxbhit` | 265 | 4 | yes |
| fs/ext4_hooks.rs | `demand_read_stats` | 268 | 4 | yes |
| fs/fdreap.rs | `retire_complete` | 132 | 38 | yes |
| fs/fdreap.rs | `drain_with_grace` | 169 | 32 |  |
| fs/fdreap.rs | `init` | 200 | 61 | yes |
| fs/file.rs | `new` | 71 | 5 | yes |
| fs/file.rs | `is_readonly` | 75 | 5 | yes |
| fs/file.rs | `is_writeonly` | 79 | 5 | yes |
| fs/file.rs | `is_rdwr` | 83 | 5 | yes |
| fs/file.rs | `bits` | 87 | 6 | yes |
| fs/file.rs | `set_bits` | 92 | 6 | yes |
| fs/file.rs | `add_flags` | 97 | 60 | yes |
| fs/file.rs | `builder` | 156 | 23 | yes |
| fs/file.rs | `read` | 178 | 6 | yes |
| fs/file.rs | `write` | 183 | 6 | yes |
| fs/file.rs | `lseek` | 188 | 6 | yes |
| fs/file.rs | `close` | 193 | 8 | yes |
| fs/file.rs | `flush` | 200 | 6 | yes |
| fs/file.rs | `poll` | 205 | 6 | yes |
| fs/file.rs | `read_at` | 210 | 7 | yes |
| fs/file.rs | `write_at` | 216 | 7 | yes |
| fs/file.rs | `fault` | 222 | 6 | yes |
| fs/file.rs | `build` | 227 | 53 | yes |
| fs/file.rs | `next_file_id` | 279 | 26 | yes |
| fs/file.rs | `file_arc_try_clone` | 304 | 32 | yes |
| fs/file.rs | `verify_arc_inner_layout` | 335 | 123 | yes |
| fs/file.rs | `none` | 457 | 26 | yes |
| fs/file.rs | `new` | 482 | 21 | yes |
| fs/file.rs | `flags` | 502 | 6 | yes |
| fs/file.rs | `set_flags` | 507 | 6 | yes |
| fs/file.rs | `flags_bits` | 512 | 13 | yes |
| fs/file.rs | `set_inode` | 524 | 12 | yes |
| fs/file.rs | `set_dentry` | 535 | 11 | yes |
| fs/file.rs | `get_dentry` | 545 | 7 | yes |
| fs/file.rs | `set_ops` | 551 | 6 | yes |
| fs/file.rs | `get_ops` | 556 | 12 | yes |
| fs/file.rs | `get_inode` | 567 | 8 | yes |
| fs/file.rs | `set_private_data` | 574 | 13 | yes |
| fs/file.rs | `private_data_ptr` | 586 | 9 | yes |
| fs/file.rs | `get_private_data` | 594 | 7 | yes |
| fs/file.rs | `clear_private_data` | 600 | 10 | yes |
| fs/file.rs | `take_private_data` | 609 | 8 | yes |
| fs/file.rs | `get_cloexec` | 616 | 6 | yes |
| fs/file.rs | `set_cloexec` | 621 | 23 | yes |
| fs/file.rs | `missing_op_errno` | 643 | 26 | yes |
| fs/file.rs | `poll_dispatch` | 668 | 20 | yes |
| fs/file.rs | `read` | 687 | 29 | yes |
| fs/file.rs | `write` | 715 | 28 | yes |
| fs/file.rs | `lseek` | 742 | 23 | yes |
| fs/file.rs | `read_at` | 764 | 75 | yes |
| fs/file.rs | `positional_capable` | 838 | 20 | yes |
| fs/file.rs | `write_at` | 857 | 91 | yes |
| fs/file.rs | `read_slice` | 947 | 8 | yes |
| fs/file.rs | `write_slice` | 954 | 8 | yes |
| fs/file.rs | `read_at_slice` | 961 | 7 | yes |
| fs/file.rs | `write_at_slice` | 967 | 9 | yes |
| fs/file.rs | `lseek_op` | 975 | 8 | yes |
| fs/file.rs | `close` | 982 | 11 | yes |
| fs/file.rs | `get_pos` | 992 | 6 | yes |
| fs/file.rs | `set_pos` | 997 | 9 | yes |
| fs/file.rs | `path` | 1005 | 36 | yes |
| fs/file.rs | `drop` | 1040 | 96 | yes |
| fs/file.rs | `new` | 1135 | 27 | yes |
| fs/file.rs | `fork_from` | 1161 | 36 | yes |
| fs/file.rs | `nofile_ceiling` | 1196 | 9 | yes |
| fs/file.rs | `alloc_fd` | 1204 | 10 | yes |
| fs/file.rs | `install_lowest_fd` | 1213 | 48 | yes |
| fs/file.rs | `alloc_fd_from` | 1260 | 30 | yes |
| fs/file.rs | `install_fd` | 1289 | 55 | yes |
| fs/file.rs | `get_file_borrow` | 1343 | 16 | yes |
| fs/file.rs | `get_file` | 1358 | 38 | yes |
| fs/file.rs | `for_each_file` | 1395 | 34 | yes |
| fs/file.rs | `close_fd` | 1428 | 71 | yes |
| fs/file.rs | `close_fd_report` | 1498 | 38 | yes |
| fs/file.rs | `park_and_drain` | 1535 | 6 | yes |
| fs/file.rs | `dup_fd` | 1540 | 16 | yes |
| fs/file.rs | `dup_fd_strict` | 1555 | 29 | yes |
| fs/file.rs | `dup2_fd` | 1583 | 49 | yes |
| fs/file.rs | `set_fd_cloexec` | 1631 | 11 | yes |
| fs/file.rs | `get_fd_cloexec` | 1641 | 8 | yes |
| fs/file.rs | `close_cloexec_fds` | 1648 | 28 | yes |
| fs/file.rs | `drop` | 1675 | 41 | yes |
| fs/file.rs | `set_cloexec_fd` | 1715 | 16 | yes |
| fs/file.rs | `alloc_staging_buf` | 1730 | 14 | yes |
| fs/file.rs | `gather_append_user` | 1743 | 23 | yes |
| fs/file.rs | `get_file_fd` | 1765 | 16 | yes |
| fs/file.rs | `file` | 1780 | 13 | yes |
| fs/file.rs | `get_file_fd_fast` | 1792 | 24 | yes |
| fs/file.rs | `get_file_fd_safe` | 1815 | 9 | yes |
| fs/file.rs | `get_file_fd_install` | 1823 | 8 | yes |
| fs/file.rs | `close_file_fd` | 1830 | 11 | yes |
| fs/file.rs | `close_file_fd_report` | 1840 | 17 | yes |
| fs/file.rs | `get_stdin` | 1856 | 5 | yes |
| fs/file.rs | `get_stdout` | 1860 | 5 | yes |
| fs/file.rs | `get_stderr` | 1864 | 9 | yes |
| fs/file.rs | `reg_file_read` | 1872 | 30 | yes |
| fs/file.rs | `reg_file_read_pub` | 1901 | 6 | yes |
| fs/file.rs | `reg_file_write_pub` | 1906 | 6 | yes |
| fs/file.rs | `reg_file_lseek_pub` | 1911 | 6 | yes |
| fs/file.rs | `reg_file_close_pub` | 1916 | 5 | yes |
| fs/file.rs | `reg_file_write` | 1920 | 68 | yes |
| fs/file.rs | `reg_file_lseek` | 1987 | 33 | yes |
| fs/file.rs | `reg_file_close` | 2019 | 22 | yes |
| fs/file.rs | `dir_read_eisdir` | 2040 | 5 | yes |
| fs/file.rs | `dir_close` | 2044 | 10 | yes |
| fs/file.rs | `dir_file_lseek` | 2053 | 24 | yes |
| fs/file_hooks.rs | `pipe_stat` | 11 | 6 | yes |
| fs/file_hooks.rs | `pipe_get_sz` | 16 | 6 | yes |
| fs/file_hooks.rs | `pipe_set_sz` | 21 | 9 | yes |
| fs/file_hooks.rs | `oasync_register` | 29 | 4 | yes |
| fs/file_hooks.rs | `oasync_unregister` | 32 | 6 | yes |
| fs/file_hooks.rs | `char_dev_stat` | 37 | 8 | yes |
| fs/file_hooks.rs | `fifo_forget` | 44 | 6 | yes |
| fs/file_hooks.rs | `fifo_open` | 49 | 10 | yes |
| fs/file_hooks.rs | `memfd_add_seals` | 58 | 4 | yes |
| fs/file_hooks.rs | `memfd_get_seals` | 61 | 4 | yes |
| fs/fs_hooks.rs | `fs_register_hooks` | 94 | 6 | yes |
| fs/fs_hooks.rs | `hooks` | 99 | 5 |  |
| fs/fs_hooks.rs | `notify` | 103 | 4 | yes |
| fs/fs_hooks.rs | `notify_file` | 106 | 4 | yes |
| fs/fs_hooks.rs | `notify_file_close` | 109 | 4 | yes |
| fs/fs_hooks.rs | `alloc_cookie` | 112 | 6 | yes |
| fs/fs_hooks.rs | `is_pipe_ops` | 117 | 2 |  |
| fs/fs_hooks.rs | `is_fifo_ops` | 118 | 2 |  |
| fs/fs_hooks.rs | `is_inotify_ops` | 119 | 2 |  |
| fs/fs_hooks.rs | `is_memfd_ops` | 120 | 2 |  |
| fs/fs_hooks.rs | `pipe_diag_snapshot` | 121 | 2 |  |
| fs/fs_hooks.rs | `memfd_name_by_ino` | 122 | 2 | yes |
| fs/fs_struct.rs | `new` | 68 | 14 | yes |
| fs/fs_struct.rs | `with_cwd` | 81 | 16 |  |
| fs/fs_struct.rs | `get_cwd` | 96 | 17 | yes |
| fs/fs_struct.rs | `set_cwd` | 112 | 9 | yes |
| fs/fs_struct.rs | `set_cwd_with_dentry` | 120 | 8 | yes |
| fs/fs_struct.rs | `cwd_dentry` | 127 | 6 | yes |
| fs/fs_struct.rs | `cwd_slice` | 132 | 10 |  |
| fs/fs_struct.rs | `get_root` | 141 | 16 | yes |
| fs/fs_struct.rs | `root_is_slash` | 156 | 11 |  |
| fs/fs_struct.rs | `set_root` | 166 | 10 |  |
| fs/fs_struct.rs | `set_root_with_dentry` | 175 | 7 | yes |
| fs/fs_struct.rs | `root_dentry` | 181 | 7 | yes |
| fs/fs_struct.rs | `set_pivot_put_old` | 187 | 6 | yes |
| fs/fs_struct.rs | `pivot_put_old` | 192 | 6 | yes |
| fs/fs_struct.rs | `clear_pivot_put_old` | 197 | 7 | yes |
| fs/fs_struct.rs | `root_is_pivot` | 203 | 6 |  |
| fs/fs_struct.rs | `set_root_is_pivot` | 208 | 10 | yes |
| fs/fs_struct.rs | `copy_fs_from` | 217 | 17 |  |
| fs/fs_struct.rs | `get_umask` | 233 | 6 | yes |
| fs/fs_struct.rs | `set_umask` | 238 | 6 | yes |
| fs/fs_struct.rs | `apply_umask` | 243 | 7 |  |
| fs/fs_struct.rs | `default` | 249 | 8 | yes |
| fs/fsctx.rs | `fs_type_known` | 111 | 32 | yes |
| fs/fsctx.rs | `param_key_known` | 142 | 80 | yes |
| fs/fsctx.rs | `new` | 221 | 16 | yes |
| fs/fsctx.rs | `new_reconfigure` | 236 | 18 | yes |
| fs/fsctx.rs | `data_string` | 253 | 40 | yes |
| fs/fsctx.rs | `create_fs_root` | 292 | 96 | yes |
| fs/fsctx.rs | `fsctx_close` | 387 | 16 |  |
| fs/fsctx.rs | `fsctx_of` | 402 | 13 | yes |
| fs/fsctx.rs | `fsctx_of_mut` | 414 | 18 | yes |
| fs/fsctx.rs | `mountfd_close` | 431 | 16 |  |
| fs/fsctx.rs | `mountfd_of` | 446 | 18 | yes |
| fs/fsctx.rs | `treepath_dentry` | 463 | 12 | yes |
| fs/fsctx.rs | `make_detached_mount` | 474 | 35 | yes |
| fs/fsctx.rs | `install_fd_with` | 508 | 25 | yes |
| fs/fsctx.rs | `mount_attr_apply` | 532 | 45 | yes |
| fs/fsctx.rs | `validate_mount_attr` | 576 | 28 | yes |
| fs/fsctx.rs | `validate_mount_attr_pair` | 603 | 31 | yes |
| fs/fsctx.rs | `attr_flags_to_mnt` | 633 | 4 | yes |
| fs/inode.rs | `new` | 128 | 5 | yes |
| fs/inode.rs | `is_regular_file` | 132 | 5 | yes |
| fs/inode.rs | `is_directory` | 136 | 5 | yes |
| fs/inode.rs | `is_char_device` | 140 | 5 | yes |
| fs/inode.rs | `is_block_device` | 144 | 5 | yes |
| fs/inode.rs | `is_fifo` | 148 | 5 | yes |
| fs/inode.rs | `is_symlink` | 152 | 5 | yes |
| fs/inode.rs | `is_socket` | 156 | 5 | yes |
| fs/inode.rs | `bits` | 160 | 164 | yes |
| fs/inode.rs | `call_lookup` | 323 | 14 | yes |
| fs/inode.rs | `call_create` | 336 | 15 | yes |
| fs/inode.rs | `call_link` | 350 | 9 | yes |
| fs/inode.rs | `call_unlink` | 358 | 9 | yes |
| fs/inode.rs | `call_symlink` | 366 | 15 | yes |
| fs/inode.rs | `call_mkdir` | 380 | 15 | yes |
| fs/inode.rs | `call_rmdir` | 394 | 9 | yes |
| fs/inode.rs | `call_mknod` | 402 | 16 | yes |
| fs/inode.rs | `call_rename` | 417 | 16 |  |
| fs/inode.rs | `call_readlink` | 432 | 9 |  |
| fs/inode.rs | `call_get_file_ops` | 440 | 14 | yes |
| fs/inode.rs | `call_readdir` | 453 | 12 | yes |
| fs/inode.rs | `call_readdir_fill` | 464 | 14 | yes |
| fs/inode.rs | `call_open` | 477 | 9 | yes |
| fs/inode.rs | `call_permission` | 485 | 9 |  |
| fs/inode.rs | `call_getattr` | 493 | 9 | yes |
| fs/inode.rs | `call_setattr` | 501 | 16 |  |
| fs/inode.rs | `call_iget` | 516 | 17 | yes |
| fs/inode.rs | `call_destroy_inode` | 532 | 73 |  |
| fs/inode.rs | `stat_dev_for_fs` | 604 | 36 |  |
| fs/inode.rs | `stat_dev_user` | 639 | 105 | yes |
| fs/inode.rs | `drop` | 743 | 24 | yes |
| fs/inode.rs | `dir_mut_gen_bump` | 766 | 7 | yes |
| fs/inode.rs | `dir_mut_gen_load` | 772 | 15 | yes |
| fs/inode.rs | `update_cached_mode` | 786 | 12 | yes |
| fs/inode.rs | `new` | 797 | 29 | yes |
| fs/inode.rs | `with_superblock` | 825 | 28 |  |
| fs/inode.rs | `read_data` | 852 | 20 | yes |
| fs/inode.rs | `append_data` | 871 | 18 | yes |
| fs/inode.rs | `write_data` | 888 | 18 | yes |
| fs/inode.rs | `load_from_bytes` | 905 | 8 | yes |
| fs/inode.rs | `set_ops` | 912 | 6 | yes |
| fs/inode.rs | `set_private_data` | 917 | 6 | yes |
| fs/inode.rs | `get_size` | 922 | 6 | yes |
| fs/inode.rs | `set_size` | 927 | 6 | yes |
| fs/inode.rs | `set_rdev` | 932 | 6 |  |
| fs/inode.rs | `get_rdev` | 937 | 7 |  |
| fs/inode.rs | `inc_ref` | 943 | 6 | yes |
| fs/inode.rs | `dec_ref` | 948 | 6 | yes |
| fs/inode.rs | `get_ref` | 953 | 16 | yes |
| fs/inode.rs | `op_lookup` | 968 | 12 | yes |
| fs/inode.rs | `op_create` | 979 | 12 |  |
| fs/inode.rs | `op_mkdir` | 990 | 12 |  |
| fs/inode.rs | `op_unlink` | 1001 | 12 |  |
| fs/inode.rs | `op_rmdir` | 1012 | 12 |  |
| fs/inode.rs | `op_link` | 1023 | 12 |  |
| fs/inode.rs | `op_symlink` | 1034 | 12 |  |
| fs/inode.rs | `op_rename` | 1045 | 12 | yes |
| fs/inode.rs | `op_readlink` | 1056 | 12 | yes |
| fs/inode.rs | `op_getattr` | 1067 | 16 | yes |
| fs/inode.rs | `op_setattr` | 1082 | 12 | yes |
| fs/inode.rs | `op_permission` | 1093 | 13 |  |
| fs/inode.rs | `make_char_inode` | 1105 | 8 |  |
| fs/inode.rs | `make_reg_inode` | 1112 | 8 |  |
| fs/inode.rs | `make_reg_inode_with_data` | 1119 | 8 |  |
| fs/inode.rs | `make_dir_inode` | 1126 | 6 |  |
| fs/inode.rs | `make_fifo_inode` | 1131 | 28 |  |
| fs/inode.rs | `new` | 1158 | 9 | yes |
| fs/inode.rs | `record_hit` | 1166 | 5 | yes |
| fs/inode.rs | `record_miss` | 1170 | 5 | yes |
| fs/inode.rs | `record_eviction` | 1174 | 5 | yes |
| fs/inode.rs | `get_hit_rate` | 1178 | 26 | yes |
| fs/inode.rs | `clone` | 1203 | 42 | yes |
| fs/inode.rs | `icache_regen` | 1244 | 13 | yes |
| fs/inode.rs | `icache_init` | 1256 | 35 |  |
| fs/inode.rs | `icache_stats_arm` | 1290 | 8 |  |
| fs/inode.rs | `inode_hash` | 1297 | 17 |  |
| fs/inode.rs | `icache_lookup` | 1313 | 41 | yes |
| fs/inode.rs | `icache_add` | 1353 | 55 | yes |
| fs/inode.rs | `icache_evict_lru` | 1407 | 39 |  |
| fs/inode.rs | `icache_remove` | 1445 | 28 | yes |
| fs/inode.rs | `icache_stats` | 1472 | 12 |  |
| fs/inode.rs | `icache_stats_detailed` | 1483 | 18 |  |
| fs/inode.rs | `icache_flush` | 1500 | 19 |  |
| fs/io_completion.rs | `new` | 87 | 22 | yes |
| fs/io_completion.rs | `begin_delivery` | 108 | 11 |  |
| fs/io_completion.rs | `end_delivery` | 118 | 10 |  |
| fs/io_completion.rs | `wait_deliveries` | 127 | 25 |  |
| fs/io_completion.rs | `complete` | 151 | 16 | yes |
| fs/io_completion.rs | `complete_with_token` | 166 | 13 |  |
| fs/io_completion.rs | `delivered_token` | 178 | 10 | yes |
| fs/io_completion.rs | `wait` | 187 | 229 | yes |
| fs/io_completion.rs | `try_wait` | 415 | 10 |  |
| fs/io_completion.rs | `is_done` | 424 | 6 |  |
| fs/io_completion.rs | `reset` | 429 | 10 | yes |
| fs/io_completion.rs | `wait_for_all` | 438 | 11 |  |
| fs/journal_seam.rs | `new` | 71 | 14 | yes |
| fs/journal_seam.rs | `begin_pre_capture` | 84 | 12 | yes |
| fs/journal_seam.rs | `take_pre_capture` | 95 | 15 | yes |
| fs/journal_seam.rs | `begin_post_capture` | 109 | 10 | yes |
| fs/journal_seam.rs | `end_post_capture` | 118 | 9 | yes |
| fs/journal_seam.rs | `pre_push` | 126 | 10 | yes |
| fs/journal_seam.rs | `current_handle_raw` | 135 | 27 | yes |
| fs/journal_seam.rs | `empty` | 161 | 5 | yes |
| fs/journal_seam.rs | `get` | 165 | 4 | yes |
| fs/journal_seam.rs | `is_some` | 168 | 4 | yes |
| fs/journal_seam.rs | `set` | 171 | 4 | yes |
| fs/journal_seam.rs | `clear` | 174 | 5 | yes |
| fs/lock_hooks.rs | `inode_open_register` | 19 | 6 | yes |
| fs/lock_hooks.rs | `inode_open_unregister` | 24 | 6 | yes |
| fs/lock_hooks.rs | `flock_release_file` | 29 | 6 | yes |
| fs/lock_hooks.rs | `ofd_release_file` | 34 | 6 | yes |
| fs/lock_hooks.rs | `break_lease` | 39 | 9 | yes |
| fs/lock_hooks.rs | `posix_test_lock` | 47 | 12 | yes |
| fs/lock_hooks.rs | `posix_set_lock` | 58 | 13 | yes |
| fs/lock_hooks.rs | `set_lease` | 70 | 7 | yes |
| fs/lock_hooks.rs | `get_lease` | 76 | 6 | yes |
| fs/lock_hooks.rs | `inode_text_hold_count` | 81 | 4 | yes |
| fs/locks.rs | `snapshot_locks` | 75 | 75 |  |
| fs/locks.rs | `dump_held_locks` | 149 | 48 |  |
| fs/locks.rs | `file_lock_key` | 196 | 12 |  |
| fs/locks.rs | `inode_lock_key` | 207 | 7 |  |
| fs/locks.rs | `current_tgid` | 213 | 40 |  |
| fs/locks.rs | `note_flock_len` | 252 | 9 |  |
| fs/locks.rs | `flock_conflicts` | 260 | 11 |  |
| fs/locks.rs | `flock_lock` | 270 | 74 | yes |
| fs/locks.rs | `flock_conflicts_locked` | 343 | 15 |  |
| fs/locks.rs | `flock_release_file` | 357 | 39 |  |
| fs/locks.rs | `report_pid` | 395 | 53 |  |
| fs/locks.rs | `note_table_len` | 447 | 17 |  |
| fs/locks.rs | `deadlock_check` | 463 | 51 |  |
| fs/locks.rs | `ranges_overlap` | 513 | 8 |  |
| fs/locks.rs | `owner_is_dead` | 520 | 10 |  |
| fs/locks.rs | `reap_stale_locked` | 529 | 20 |  |
| fs/locks.rs | `find_conflict` | 548 | 29 |  |
| fs/locks.rs | `posix_test_lock` | 576 | 22 |  |
| fs/locks.rs | `apply_owner_lock` | 597 | 72 |  |
| fs/locks.rs | `posix_set_lock` | 668 | 107 |  |
| fs/locks.rs | `posix_release_on_close` | 774 | 24 |  |
| fs/locks.rs | `ofd_release_file` | 797 | 18 |  |
| fs/locks.rs | `posix_release_for_file` | 814 | 12 |  |
| fs/locks.rs | `record_lock_count` | 825 | 7 |  |
| fs/locks.rs | `flock_lock_count` | 831 | 55 |  |
| fs/locks.rs | `note_lease_len` | 885 | 24 |  |
| fs/locks.rs | `inode_open_register` | 908 | 14 |  |
| fs/locks.rs | `inode_open_unregister` | 921 | 39 |  |
| fs/locks.rs | `inode_write_open_count` | 959 | 16 | yes |
| fs/locks.rs | `inode_text_hold_key` | 974 | 6 | yes |
| fs/locks.rs | `inode_text_hold_acquire_key` | 979 | 8 | yes |
| fs/locks.rs | `inode_text_hold_release` | 986 | 14 |  |
| fs/locks.rs | `inode_text_hold_count` | 999 | 9 |  |
| fs/locks.rs | `set_lease` | 1007 | 101 |  |
| fs/locks.rs | `get_lease` | 1107 | 29 |  |
| fs/locks.rs | `lease_conflicts` | 1135 | 22 |  |
| fs/locks.rs | `break_lease` | 1156 | 73 |  |
| fs/mod.rs | `read_file_from_rootfs` | 72 | 31 | yes |
| fs/mount.rs | `register_mount` | 41 | 21 | yes |
| fs/mount.rs | `mounted_as` | 61 | 10 |  |
| fs/mount.rs | `get_global_mounts` | 70 | 15 |  |
| fs/mount.rs | `get_mounts` | 84 | 12 | yes |
| fs/mount.rs | `get_mount_entries` | 95 | 25 |  |
| fs/mount.rs | `new` | 119 | 5 | yes |
| fs/mount.rs | `is_readonly` | 123 | 5 | yes |
| fs/mount.rs | `is_noexec` | 127 | 5 |  |
| fs/mount.rs | `is_nosuid` | 131 | 5 |  |
| fs/mount.rs | `bits` | 135 | 22 | yes |
| fs/mount.rs | `new` | 156 | 13 | yes |
| fs/mount.rs | `get_superblock` | 168 | 5 |  |
| fs/mount.rs | `set_superblock` | 172 | 20 |  |
| fs/mount.rs | `translate_mnt_flags` | 191 | 14 |  |
| fs/mount.rs | `translate_mnt_flags_public` | 204 | 13 | yes |
| fs/mount.rs | `do_mount` | 216 | 38 | yes |
| fs/mount_table.rs | `register` | 33 | 16 | yes |
| fs/mount_table.rs | `lookup` | 48 | 16 | yes |
| fs/mount_table.rs | `register_builtin_handlers` | 63 | 20 |  |
| fs/notify_hooks.rs | `notify` | 22 | 12 | yes |
| fs/notify_hooks.rs | `notify_file` | 33 | 6 | yes |
| fs/notify_hooks.rs | `notify_file_close` | 38 | 6 | yes |
| fs/notify_hooks.rs | `alloc_cookie` | 43 | 4 | yes |
| fs/page_cache.rs | `max_cached_pages` | 105 | 84 | yes |
| fs/page_cache.rs | `dirty_account_transition` | 188 | 15 | yes |
| fs/page_cache.rs | `dirty_account_removed` | 202 | 8 | yes |
| fs/page_cache.rs | `dirty_pages` | 209 | 17 | yes |
| fs/page_cache.rs | `len` | 225 | 48 | yes |
| fs/page_cache.rs | `pcache_reclaim_census_line` | 272 | 30 | yes |
| fs/page_cache.rs | `pcache_dirty_histogram` | 301 | 94 | yes |
| fs/page_cache.rs | `pin_locked` | 394 | 7 | yes |
| fs/page_cache.rs | `unpin_locked` | 400 | 10 | yes |
| fs/page_cache.rs | `count_locked` | 409 | 6 | yes |
| fs/page_cache.rs | `take_released_locked` | 414 | 84 | yes |
| fs/page_cache.rs | `new` | 497 | 40 | yes |
| fs/page_cache.rs | `claim_entry_arc` | 536 | 15 | yes |
| fs/page_cache.rs | `cache_key` | 550 | 7 | yes |
| fs/page_cache.rs | `phys_to_virt_ptr` | 556 | 18 | yes |
| fs/page_cache.rs | `from_phys` | 573 | 9 | yes |
| fs/page_cache.rs | `fill_from` | 581 | 15 | yes |
| fs/page_cache.rs | `copy_frame_from` | 595 | 27 | yes |
| fs/page_cache.rs | `fill_page_from` | 621 | 7 | yes |
| fs/page_cache.rs | `copy_page_page` | 627 | 8 | yes |
| fs/page_cache.rs | `copy_into_page` | 634 | 14 | yes |
| fs/page_cache.rs | `copy_user_into_page` | 647 | 15 | yes |
| fs/page_cache.rs | `new` | 661 | 48 | yes |
| fs/page_cache.rs | `lookup_claimed` | 708 | 40 | yes |
| fs/page_cache.rs | `probe_slot_ptr` | 747 | 25 | yes |
| fs/page_cache.rs | `table_find` | 771 | 24 | yes |
| fs/page_cache.rs | `table_find_ptr` | 794 | 27 | yes |
| fs/page_cache.rs | `table_publish` | 820 | 51 | yes |
| fs/page_cache.rs | `remove_entry` | 870 | 25 | yes |
| fs/page_cache.rs | `park_and_drain` | 894 | 11 | yes |
| fs/page_cache.rs | `flush_parked` | 904 | 25 | yes |
| fs/page_cache.rs | `drain_retired` | 928 | 54 | yes |
| fs/page_cache.rs | `get` | 981 | 36 | yes |
| fs/page_cache.rs | `resident` | 1016 | 18 | yes |
| fs/page_cache.rs | `get_phys` | 1033 | 30 | yes |
| fs/page_cache.rs | `get_stale` | 1062 | 33 | yes |
| fs/page_cache.rs | `get_stale_revive` | 1094 | 48 | yes |
| fs/page_cache.rs | `get_range_into` | 1141 | 20 | yes |
| fs/page_cache.rs | `get_range_with_entry` | 1160 | 67 | yes |
| fs/page_cache.rs | `unpin_contiguous_locked` | 1226 | 62 | yes |
| fs/page_cache.rs | `put_range` | 1287 | 56 | yes |
| fs/page_cache.rs | `put_range_with_entry` | 1342 | 43 | yes |
| fs/page_cache.rs | `put` | 1384 | 50 | yes |
| fs/page_cache.rs | `put_by_key` | 1433 | 40 | yes |
| fs/page_cache.rs | `put_range_by_key` | 1472 | 55 | yes |
| fs/page_cache.rs | `pin_by_key` | 1526 | 49 | yes |
| fs/page_cache.rs | `mark_dirty` | 1574 | 7 | yes |
| fs/page_cache.rs | `mark_dirty_by_key` | 1580 | 21 | yes |
| fs/page_cache.rs | `mark_dirty_pfn` | 1600 | 40 | yes |
| fs/page_cache.rs | `writeback_update_range` | 1639 | 19 | yes |
| fs/page_cache.rs | `writeback_update_range_multi` | 1657 | 19 | yes |
| fs/page_cache.rs | `writeback_update_range_pinned` | 1675 | 65 | yes |
| fs/page_cache.rs | `writeback_update_range_stale` | 1739 | 11 | yes |
| fs/page_cache.rs | `writeback_update_range_impl` | 1749 | 78 | yes |
| fs/page_cache.rs | `writeback_populate_range` | 1826 | 14 | yes |
| fs/page_cache.rs | `writeback_populate_range_multi` | 1839 | 59 | yes |
| fs/page_cache.rs | `collect_dirty_range` | 1897 | 48 | yes |
| fs/page_cache.rs | `dirty_extent_hint` | 1944 | 24 | yes |
| fs/page_cache.rs | `dirty_tail_eof` | 1967 | 24 | yes |
| fs/page_cache.rs | `note_dirty_eof` | 1990 | 42 | yes |
| fs/page_cache.rs | `flush_all_dirty` | 2031 | 79 | yes |
| fs/page_cache.rs | `flush_done` | 2109 | 76 | yes |
| fs/page_cache.rs | `invalidate_inode` | 2184 | 67 | yes |
| fs/page_cache.rs | `invalidate_inode_range` | 2250 | 20 | yes |
| fs/page_cache.rs | `invalidate_inode_span` | 2269 | 72 | yes |
| fs/page_cache.rs | `insert` | 2340 | 23 | yes |
| fs/page_cache.rs | `insert_dirty` | 2362 | 16 | yes |
| fs/page_cache.rs | `insert_dirty_user` | 2377 | 133 | yes |
| fs/page_cache.rs | `insert_ext` | 2509 | 231 | yes |
| fs/page_cache.rs | `insert_batch` | 2739 | 22 | yes |
| fs/page_cache.rs | `insert_dirty_batch` | 2760 | 26 | yes |
| fs/page_cache.rs | `insert_dirty_batch_user` | 2785 | 25 | yes |
| fs/page_cache.rs | `insert_batch_ext` | 2809 | 21 | yes |
| fs/page_cache.rs | `discard_prepared_frame` | 2829 | 399 | yes |
| fs/page_cache.rs | `shrink` | 3227 | 41 | yes |
| fs/page_cache.rs | `collect_wb_work` | 3267 | 68 | yes |
| fs/page_cache.rs | `evict_clean_inode` | 3334 | 86 | yes |
| fs/page_cache.rs | `writeback_recover` | 3419 | 92 | yes |
| fs/page_cache.rs | `shrink_reclaim` | 3510 | 129 | yes |
| fs/page_cache.rs | `evict_one_locked` | 3638 | 246 | yes |
| fs/page_cache.rs | `release_page_frame` | 3883 | 61 | yes |
| fs/page_cache.rs | `get_page_cache` | 3943 | 6 | yes |
| fs/page_cache.rs | `page_cache_total_pages` | 3948 | 9 | yes |
| fs/page_cache.rs | `page_cache_stats` | 3956 | 12 | yes |
| fs/page_cache.rs | `page_cache_cap_pages` | 3967 | 25 | yes |
| fs/page_cache.rs | `shared_file_fault_page` | 3991 | 51 | yes |
| fs/path.rs | `new` | 55 | 9 | yes |
| fs/path.rs | `name` | 63 | 6 | yes |
| fs/path.rs | `is_current` | 68 | 6 |  |
| fs/path.rs | `is_parent` | 73 | 6 |  |
| fs/path.rs | `is_root` | 78 | 6 |  |
| fs/path.rs | `is_empty` | 83 | 14 | yes |
| fs/path.rs | `new` | 96 | 6 | yes |
| fs/path.rs | `is_absolute` | 101 | 6 |  |
| fs/path.rs | `is_empty` | 106 | 6 | yes |
| fs/path.rs | `as_str` | 111 | 6 | yes |
| fs/path.rs | `components` | 116 | 9 |  |
| fs/path.rs | `parent` | 124 | 14 |  |
| fs/path.rs | `file_name` | 137 | 16 |  |
| fs/path.rs | `join` | 152 | 26 | yes |
| fs/path.rs | `next` | 177 | 22 | yes |
| fs/path.rs | `filename_parentname` | 198 | 22 |  |
| fs/path.rs | `path_normalize` | 219 | 72 | yes |
| fs/permission.rs | `generic_permission` | 22 | 82 | yes |
| fs/permission.rs | `inode_permission` | 103 | 18 | yes |
| fs/pidfd.rs | `exit_notify_all_waiters` | 54 | 16 |  |
| fs/pidfd.rs | `pin_leader` | 69 | 25 |  |
| fs/pidfd.rs | `make_pidfd_data` | 93 | 7 | yes |
| fs/pidfd.rs | `free_pidfd_data` | 99 | 13 |  |
| fs/pidfd.rs | `attach` | 111 | 10 | yes |
| fs/pidfd.rs | `data_of` | 120 | 19 |  |
| fs/pidfd.rs | `fdinfo_pid_of` | 138 | 15 |  |
| fs/pidfd.rs | `resolve_alive` | 152 | 15 | yes |
| fs/pidfd.rs | `target_exited` | 166 | 15 |  |
| fs/pidfd.rs | `pidfd_poll` | 180 | 10 |  |
| fs/pidfd.rs | `pidfd_close` | 189 | 18 |  |
| fs/pidfd.rs | `pidfd_poll_wait` | 206 | 10 |  |
| fs/pidfd.rs | `pidfd_poll_unwait` | 215 | 10 |  |
| fs/pidfd.rs | `pidfd_dir_close` | 224 | 6 |  |
| fs/pidfd.rs | `pidfd_dir_poll` | 229 | 4 |  |
| fs/procfs/cmdline.rs | `generate` | 17 | 16 |  |
| fs/procfs/cmdline.rs | `get_cmdline` | 32 | 9 | yes |
| fs/procfs/configgz.rs | `crc32_ieee` | 38 | 17 |  |
| fs/procfs/configgz.rs | `generate` | 54 | 21 |  |
| fs/procfs/cpuinfo.rs | `generate` | 63 | 70 |  |
| fs/procfs/diskstats.rs | `generate` | 32 | 66 |  |
| fs/procfs/interrupts.rs | `timer_inc` | 38 | 8 | yes |
| fs/procfs/interrupts.rs | `soft_inc` | 45 | 12 | yes |
| fs/procfs/interrupts.rs | `timer_count` | 56 | 10 | yes |
| fs/procfs/interrupts.rs | `soft_count` | 65 | 11 |  |
| fs/procfs/interrupts.rs | `total_interrupt_count` | 75 | 22 |  |
| fs/procfs/interrupts.rs | `plic_inc` | 96 | 6 |  |
| fs/procfs/interrupts.rs | `plic_count` | 101 | 10 |  |
| fs/procfs/interrupts.rs | `generate` | 110 | 67 |  |
| fs/procfs/kallsyms.rs | `generate` | 45 | 19 |  |
| fs/procfs/loadavg.rs | `update_load_avg` | 73 | 41 | yes |
| fs/procfs/loadavg.rs | `fmt_load` | 113 | 10 |  |
| fs/procfs/loadavg.rs | `generate` | 122 | 27 |  |
| fs/procfs/locks.rs | `generate` | 33 | 7 |  |
| fs/procfs/locks.rs | `render` | 39 | 34 |  |
| fs/procfs/meminfo.rs | `mem_total_kb` | 16 | 6 |  |
| fs/procfs/meminfo.rs | `mem_free_kb` | 21 | 6 |  |
| fs/procfs/meminfo.rs | `mem_available_kb` | 26 | 6 |  |
| fs/procfs/meminfo.rs | `mem_used_kb` | 31 | 6 |  |
| fs/procfs/meminfo.rs | `cached_kb` | 36 | 16 |  |
| fs/procfs/meminfo.rs | `lru_kb` | 51 | 22 |  |
| fs/procfs/meminfo.rs | `swap_kb` | 72 | 10 |  |
| fs/procfs/meminfo.rs | `heap_kb` | 81 | 10 |  |
| fs/procfs/meminfo.rs | `page_cache_ops_line` | 90 | 14 |  |
| fs/procfs/meminfo.rs | `active_inactive_kb` | 103 | 91 |  |
| fs/procfs/meminfo.rs | `generate` | 193 | 5 |  |
| fs/procfs/mod.rs | `attr_file_kind_of` | 97 | 26 |  |
| fs/procfs/mod.rs | `is_fdinfo_dir` | 122 | 7 |  |
| fs/procfs/mod.rs | `fdinfo_file_of` | 128 | 13 |  |
| fs/procfs/mod.rs | `is_task_dir` | 140 | 7 |  |
| fs/procfs/mod.rs | `is_attr_dir` | 146 | 125 |  |
| fs/procfs/mod.rs | `new_dir` | 270 | 24 |  |
| fs/procfs/mod.rs | `new_dynamic_file` | 293 | 24 |  |
| fs/procfs/mod.rs | `new_static_file` | 316 | 25 |  |
| fs/procfs/mod.rs | `new_dynamic_symlink` | 340 | 25 |  |
| fs/procfs/mod.rs | `new_sysctl_file` | 364 | 29 |  |
| fs/procfs/mod.rs | `new_symlink` | 392 | 25 |  |
| fs/procfs/mod.rs | `is_dir` | 416 | 6 | yes |
| fs/procfs/mod.rs | `is_file` | 421 | 6 |  |
| fs/procfs/mod.rs | `is_symlink` | 426 | 6 | yes |
| fs/procfs/mod.rs | `get_content` | 431 | 50 |  |
| fs/procfs/mod.rs | `get_link_target` | 480 | 21 |  |
| fs/procfs/mod.rs | `size` | 500 | 10 | yes |
| fs/procfs/mod.rs | `find_child` | 509 | 27 |  |
| fs/procfs/mod.rs | `add_child` | 535 | 7 |  |
| fs/procfs/mod.rs | `list_children` | 541 | 10 |  |
| fs/procfs/mod.rs | `get` | 550 | 8 | yes |
| fs/procfs/mod.rs | `put` | 557 | 25 | yes |
| fs/procfs/mod.rs | `new` | 581 | 13 | yes |
| fs/procfs/mod.rs | `alloc_ino` | 593 | 6 |  |
| fs/procfs/mod.rs | `init_default_files` | 598 | 102 |  |
| fs/procfs/mod.rs | `init_net_tree` | 699 | 58 |  |
| fs/procfs/mod.rs | `init_sys_tree` | 756 | 319 |  |
| fs/procfs/mod.rs | `init_sysvipc_tree` | 1074 | 22 |  |
| fs/procfs/mod.rs | `create_dynamic_file` | 1095 | 12 |  |
| fs/procfs/mod.rs | `create_static_file` | 1106 | 12 |  |
| fs/procfs/mod.rs | `create_dynamic_symlink` | 1117 | 12 |  |
| fs/procfs/mod.rs | `create_symlink` | 1128 | 12 |  |
| fs/procfs/mod.rs | `lookup` | 1139 | 43 | yes |
| fs/procfs/mod.rs | `read_file` | 1181 | 13 | yes |
| fs/procfs/mod.rs | `list_dir` | 1193 | 34 | yes |
| fs/procfs/mod.rs | `procfs_mount` | 1226 | 10 |  |
| fs/procfs/mod.rs | `procfs_kill_sb` | 1235 | 9 |  |
| fs/procfs/mod.rs | `get_procfs_sb` | 1243 | 12 |  |
| fs/procfs/mod.rs | `mount_table_regen` | 1254 | 25 | yes |
| fs/procfs/mod.rs | `read_file` | 1278 | 120 | yes |
| fs/procfs/mod.rs | `is_mounted` | 1397 | 8 | yes |
| fs/procfs/mod.rs | `create_root_inode` | 1404 | 26 | yes |
| fs/procfs/mod.rs | `list_dir` | 1429 | 6 | yes |
| fs/procfs/mod.rs | `exists` | 1434 | 13 |  |
| fs/procfs/mod.rs | `init_procfs` | 1446 | 23 |  |
| fs/procfs/mod.rs | `mount_procfs` | 1468 | 37 |  |
| fs/procfs/mod.rs | `procfs_lookup` | 1504 | 134 |  |
| fs/procfs/mod.rs | `procfs_getattr` | 1637 | 204 |  |
| fs/procfs/mod.rs | `procfs_readlink` | 1840 | 48 |  |
| fs/procfs/mod.rs | `procfs_iget` | 1887 | 290 |  |
| fs/procfs/mod.rs | `procfs_file_read` | 2176 | 23 |  |
| fs/procfs/mod.rs | `procfs_file_write` | 2198 | 92 |  |
| fs/procfs/mod.rs | `procfs_file_lseek` | 2289 | 54 |  |
| fs/procfs/mod.rs | `procfs_file_close` | 2342 | 15 |  |
| fs/procfs/mod.rs | `procfs_setattr` | 2356 | 43 |  |
| fs/procfs/mod.rs | `procfs_file_poll` | 2398 | 29 |  |
| fs/procfs/mod.rs | `proc_piddir_close` | 2426 | 5 |  |
| fs/procfs/mod.rs | `proc_piddir_poll` | 2430 | 14 |  |
| fs/procfs/mod.rs | `procfs_get_file_ops` | 2443 | 34 |  |
| fs/procfs/mod.rs | `procfs_open` | 2476 | 57 |  |
| fs/procfs/mod.rs | `generate_pid_dir_entries` | 2532 | 53 |  |
| fs/procfs/mod.rs | `procfs_readdir` | 2584 | 153 |  |
| fs/procfs/mod.rs | `procfs_ns_dir_lookup` | 2736 | 15 |  |
| fs/procfs/mod.rs | `procfs_ns_dir_iget` | 2750 | 16 |  |
| fs/procfs/mod.rs | `procfs_ns_dir_readdir` | 2765 | 29 |  |
| fs/procfs/mod.rs | `procfs_ns_dir_getattr` | 2793 | 24 |  |
| fs/procfs/mod.rs | `procfs_fd_dir_lookup` | 2816 | 34 |  |
| fs/procfs/mod.rs | `procfs_fd_dir_iget` | 2849 | 45 |  |
| fs/procfs/mod.rs | `generate_slabinfo` | 2893 | 10 |  |
| fs/procfs/mod.rs | `generate_vmallocinfo` | 2902 | 5 |  |
| fs/procfs/mod.rs | `partitions_generate` | 2906 | 25 |  |
| fs/procfs/mod.rs | `mount_proc` | 2930 | 9 |  |
| fs/procfs/mod.rs | `mount_register` | 2938 | 4 | yes |
| fs/procfs/mounts.rs | `boot_mounts` | 45 | 58 |  |
| fs/procfs/mounts.rs | `generate` | 102 | 45 |  |
| fs/procfs/mounts.rs | `generate_filesystems` | 146 | 19 |  |
| fs/procfs/mounts.rs | `mount_point_prefix` | 164 | 18 |  |
| fs/procfs/mounts.rs | `mount_id_for` | 181 | 32 | yes |
| fs/procfs/mounts.rs | `generate_mountinfo` | 212 | 51 |  |
| fs/procfs/net.rs | `hex_addr_v4` | 27 | 12 |  |
| fs/procfs/net.rs | `hex_port` | 38 | 8 |  |
| fs/procfs/net.rs | `hex_addr_v6` | 45 | 17 |  |
| fs/procfs/net.rs | `dev_rows` | 61 | 12 | yes |
| fs/procfs/net.rs | `generate_dev` | 72 | 41 |  |
| fs/procfs/net.rs | `sock_table_header` | 112 | 7 |  |
| fs/procfs/net.rs | `sock_row` | 118 | 22 |  |
| fs/procfs/net.rs | `generate_tcp` | 139 | 22 |  |
| fs/procfs/net.rs | `generate_tcp6` | 160 | 23 |  |
| fs/procfs/net.rs | `generate_udp` | 182 | 22 |  |
| fs/procfs/net.rs | `generate_udp6` | 203 | 26 |  |
| fs/procfs/net.rs | `generate_arp` | 228 | 17 |  |
| fs/procfs/net.rs | `format_mac` | 244 | 9 |  |
| fs/procfs/net.rs | `format_v4` | 252 | 16 |  |
| fs/procfs/net.rs | `generate_route` | 267 | 32 |  |
| fs/procfs/net.rs | `generate_fib_trie` | 298 | 24 |  |
| fs/procfs/net.rs | `generate_sockstat` | 321 | 41 |  |
| fs/procfs/net.rs | `generate_unix` | 361 | 9 |  |
| fs/procfs/pid.rs | `is_pid_dir` | 25 | 9 |  |
| fs/procfs/pid.rs | `is_valid_pid` | 33 | 11 | yes |
| fs/procfs/pid.rs | `parse_pid` | 43 | 23 |  |
| fs/procfs/pid.rs | `task_state_char` | 65 | 14 |  |
| fs/procfs/pid.rs | `task_state_str` | 78 | 23 |  |
| fs/procfs/pid.rs | `task_comm` | 100 | 49 |  |
| fs/procfs/pid.rs | `lookup` | 148 | 5 | yes |
| fs/procfs/pid.rs | `get` | 152 | 9 | yes |
| fs/procfs/pid.rs | `drop` | 160 | 7 | yes |
| fs/procfs/pid.rs | `generate_comm` | 166 | 61 |  |
| fs/procfs/pid.rs | `generate_cgroup` | 226 | 33 |  |
| fs/procfs/pid.rs | `generate_status` | 258 | 201 |  |
| fs/procfs/pid.rs | `read_target_user_mm` | 458 | 49 |  |
| fs/procfs/pid.rs | `environ_access_allowed` | 506 | 14 | yes |
| fs/procfs/pid.rs | `generate_cmdline` | 519 | 46 |  |
| fs/procfs/pid.rs | `generate_schedstat` | 564 | 23 |  |
| fs/procfs/pid.rs | `generate_stat` | 586 | 135 |  |
| fs/procfs/pid.rs | `generate_exe_link` | 720 | 25 | yes |
| fs/procfs/pid.rs | `generate_cwd_link` | 744 | 25 |  |
| fs/procfs/pid.rs | `generate_environ` | 768 | 46 |  |
| fs/procfs/pid.rs | `generate_smaps` | 813 | 84 |  |
| fs/procfs/pid.rs | `re_header` | 896 | 15 |  |
| fs/procfs/pid.rs | `generate_maps` | 910 | 169 |  |
| fs/procfs/pid.rs | `get_fd_file` | 1078 | 19 | yes |
| fs/procfs/pid.rs | `list_fds` | 1096 | 32 | yes |
| fs/procfs/pid.rs | `get_fd_path` | 1127 | 31 |  |
| fs/procfs/pid.rs | `generate_fd_link` | 1157 | 33 | yes |
| fs/procfs/pid.rs | `generate_oom_score` | 1189 | 40 |  |
| fs/procfs/pid.rs | `generate_oom_score_adj` | 1228 | 47 |  |
| fs/procfs/pid.rs | `trim_label` | 1274 | 40 |  |
| fs/procfs/pid.rs | `attr_get` | 1313 | 37 |  |
| fs/procfs/pid.rs | `attr_set` | 1349 | 12 |  |
| fs/procfs/pid.rs | `attr_label_get` | 1360 | 6 |  |
| fs/procfs/pid.rs | `attr_label_set` | 1365 | 10 |  |
| fs/procfs/pid.rs | `generate_fdinfo` | 1374 | 50 |  |
| fs/procfs/procfile.rs | `render_kv` | 67 | 20 |  |
| fs/procfs/procfile.rs | `render_pad` | 86 | 29 |  |
| fs/procfs/procfile.rs | `render_kvtab` | 114 | 16 |  |
| fs/procfs/procfile.rs | `render_kvtab_pad` | 129 | 23 |  |
| fs/procfs/procfile.rs | `render_cols` | 151 | 87 |  |
| fs/procfs/schedstat.rs | `note_yield` | 52 | 8 | yes |
| fs/procfs/schedstat.rs | `note_sched_run` | 59 | 8 | yes |
| fs/procfs/schedstat.rs | `generate` | 66 | 54 |  |
| fs/procfs/self_proc.rs | `get_self_link` | 16 | 8 |  |
| fs/procfs/self_proc.rs | `generate` | 23 | 10 |  |
| fs/procfs/self_proc.rs | `get_thread_self_link` | 32 | 11 |  |
| fs/procfs/softirqs.rs | `generate` | 36 | 27 |  |
| fs/procfs/softirqs.rs | `total_raised` | 62 | 14 |  |
| fs/procfs/softirqs.rs | `raised_count` | 75 | 4 |  |
| fs/procfs/stat.rs | `new` | 61 | 31 | yes |
| fs/procfs/stat.rs | `charge_tick` | 91 | 25 | yes |
| fs/procfs/stat.rs | `note_switch` | 115 | 23 | yes |
| fs/procfs/stat.rs | `note_fork` | 137 | 6 |  |
| fs/procfs/stat.rs | `total_context_switches` | 142 | 6 |  |
| fs/procfs/stat.rs | `total_forks` | 147 | 6 |  |
| fs/procfs/stat.rs | `procs_running` | 152 | 6 |  |
| fs/procfs/stat.rs | `procs_blocked` | 157 | 20 |  |
| fs/procfs/stat.rs | `btime` | 176 | 15 |  |
| fs/procfs/stat.rs | `generate` | 190 | 139 |  |
| fs/procfs/swaps.rs | `generate` | 20 | 21 |  |
| fs/procfs/sysctl.rs | `new` | 84 | 12 | yes |
| fs/procfs/sysctl.rs | `as_bytes` | 95 | 14 | yes |
| fs/procfs/sysctl.rs | `generate_hostname` | 108 | 9 |  |
| fs/procfs/sysctl.rs | `generate_domainname` | 116 | 9 |  |
| fs/procfs/sysctl.rs | `generate_osrelease` | 124 | 5 |  |
| fs/procfs/sysctl.rs | `generate_pid_max` | 128 | 5 |  |
| fs/procfs/sysctl.rs | `generate_ostype` | 132 | 13 |  |
| fs/procfs/sysctl.rs | `generate_tainted` | 144 | 10 |  |
| fs/procfs/sysctl.rs | `generate_panic_on_oops` | 153 | 13 |  |
| fs/procfs/sysctl.rs | `generate_cov_dump` | 165 | 6 |  |
| fs/procfs/sysctl.rs | `write_cov_dump` | 170 | 5 |  |
| fs/procfs/sysctl.rs | `write_panic_on_oops` | 174 | 15 |  |
| fs/procfs/sysctl.rs | `generate_sched_schedstats` | 188 | 9 |  |
| fs/procfs/sysctl.rs | `write_sched_schedstats` | 196 | 13 |  |
| fs/procfs/sysctl.rs | `generate_cap_last_cap` | 208 | 5 |  |
| fs/procfs/sysctl.rs | `generate_overcommit_memory` | 212 | 5 |  |
| fs/procfs/sysctl.rs | `generate_file_max` | 216 | 5 |  |
| fs/procfs/sysctl.rs | `generate_pipe_user_pages_soft` | 220 | 5 |  |
| fs/procfs/sysctl.rs | `generate_pipe_user_pages_hard` | 224 | 10 |  |
| fs/procfs/sysctl.rs | `generate_pipe_max_size` | 233 | 5 |  |
| fs/procfs/sysctl.rs | `write_pipe_max_size` | 237 | 18 |  |
| fs/procfs/sysctl.rs | `generate_lease_break_time` | 254 | 9 |  |
| fs/procfs/sysctl.rs | `write_lease_break_time` | 262 | 20 |  |
| fs/procfs/sysctl.rs | `parse_u64` | 281 | 21 |  |
| fs/procfs/sysctl.rs | `write_hostname` | 301 | 19 |  |
| fs/procfs/sysctl.rs | `write_domainname` | 319 | 19 |  |
| fs/procfs/sysctl.rs | `write_pid_max` | 337 | 16 |  |
| fs/procfs/sysctl.rs | `write_overcommit_memory` | 352 | 14 |  |
| fs/procfs/sysctl.rs | `write_file_max` | 365 | 11 |  |
| fs/procfs/sysctl.rs | `write_pipe_user_pages_soft` | 375 | 11 |  |
| fs/procfs/sysctl.rs | `write_pipe_user_pages_hard` | 385 | 13 |  |
| fs/procfs/sysctl.rs | `generate_msgmni` | 397 | 7 |  |
| fs/procfs/sysctl.rs | `write_msgmni` | 403 | 15 |  |
| fs/procfs/sysctl.rs | `generate_threads_max` | 417 | 12 |  |
| fs/procfs/sysctl.rs | `generate_printk` | 428 | 9 |  |
| fs/procfs/sysctl.rs | `write_printk` | 436 | 18 |  |
| fs/procfs/sysctl.rs | `generate_printk_devkmsg` | 453 | 12 |  |
| fs/procfs/sysctl.rs | `write_printk_devkmsg` | 464 | 18 |  |
| fs/procfs/sysctl.rs | `generate_shmmax` | 481 | 5 |  |
| fs/procfs/sysctl.rs | `write_shmmax` | 485 | 13 |  |
| fs/procfs/sysctl.rs | `generate_shmmni` | 497 | 5 |  |
| fs/procfs/sysctl.rs | `write_shmmni` | 501 | 14 |  |
| fs/procfs/sysctl.rs | `generate_shmall` | 514 | 5 |  |
| fs/procfs/sysctl.rs | `write_shmall` | 518 | 12 |  |
| fs/procfs/sysctl.rs | `generate_mq_queues_max` | 529 | 5 |  |
| fs/procfs/sysctl.rs | `write_mq_queues_max` | 533 | 14 |  |
| fs/procfs/sysctl.rs | `generate_mq_msg_max` | 546 | 6 |  |
| fs/procfs/sysctl.rs | `generate_mq_msgsize_max` | 551 | 6 |  |
| fs/procfs/sysctl.rs | `generate_msgmax` | 556 | 6 |  |
| fs/procfs/sysctl.rs | `generate_msgmnb` | 561 | 5 |  |
| fs/procfs/sysctl.rs | `generate_core_pattern` | 565 | 7 |  |
| fs/procfs/sysctl.rs | `write_core_pattern` | 571 | 31 |  |
| fs/procfs/sysctl.rs | `generate_inotify_max_queued_events` | 601 | 5 |  |
| fs/procfs/sysctl.rs | `write_inotify_max_queued_events` | 605 | 21 |  |
| fs/procfs/sysctl.rs | `generate_inotify_max_user_instances` | 625 | 5 |  |
| fs/procfs/sysctl.rs | `write_inotify_max_user_instances` | 629 | 16 |  |
| fs/procfs/sysctl.rs | `generate_inotify_max_user_watches` | 644 | 5 |  |
| fs/procfs/sysctl.rs | `write_inotify_max_user_watches` | 648 | 15 |  |
| fs/procfs/sysctl.rs | `generate_ns_last_pid` | 662 | 5 |  |
| fs/procfs/sysctl.rs | `write_ns_last_pid` | 666 | 31 |  |
| fs/procfs/sysctl.rs | `generate_perf_event_paranoid` | 696 | 4 |  |
| fs/procfs/sysctl.rs | `write_perf_event_paranoid` | 699 | 10 |  |
| fs/procfs/sysctl.rs | `generate_perf_max_sample_rate` | 708 | 4 |  |
| fs/procfs/sysctl.rs | `write_perf_max_sample_rate` | 711 | 10 |  |
| fs/procfs/sysctl.rs | `generate_perf_cpu_time_max_percent` | 720 | 4 |  |
| fs/procfs/sysctl.rs | `write_perf_cpu_time_max_percent` | 723 | 10 |  |
| fs/procfs/sysctl.rs | `generate_perf_event_mlock_kb` | 732 | 4 |  |
| fs/procfs/sysctl.rs | `write_perf_event_mlock_kb` | 735 | 20 |  |
| fs/procfs/sysctl.rs | `generate_key_quota` | 754 | 5 |  |
| fs/procfs/sysctl.rs | `write_key_quota` | 758 | 11 |  |
| fs/procfs/sysctl.rs | `generate_root_maxkeys` | 768 | 4 |  |
| fs/procfs/sysctl.rs | `write_root_maxkeys` | 771 | 4 |  |
| fs/procfs/sysctl.rs | `generate_root_maxbytes` | 774 | 4 |  |
| fs/procfs/sysctl.rs | `write_root_maxbytes` | 777 | 4 |  |
| fs/procfs/sysctl.rs | `generate_maxkeys` | 780 | 4 |  |
| fs/procfs/sysctl.rs | `write_maxkeys` | 783 | 4 |  |
| fs/procfs/sysctl.rs | `generate_maxbytes` | 786 | 4 |  |
| fs/procfs/sysctl.rs | `write_maxbytes` | 789 | 15 |  |
| fs/procfs/sysctl.rs | `format_uuid_v4` | 803 | 13 |  |
| fs/procfs/sysctl.rs | `generate_boot_id` | 815 | 13 |  |
| fs/procfs/sysctl.rs | `generate_uuid` | 827 | 6 |  |
| fs/procfs/sysrq.rs | `run_command` | 30 | 102 |  |
| fs/procfs/sysrq.rs | `signal_all_except_init` | 131 | 19 |  |
| fs/procfs/sysrq.rs | `dump_all_cpu_stacks` | 149 | 26 |  |
| fs/procfs/sysrq.rs | `print_task_stack_words` | 174 | 46 |  |
| fs/procfs/sysrq.rs | `dump_blocked_tasks` | 219 | 28 |  |
| fs/procfs/sysrq.rs | `print_help` | 246 | 21 |  |
| fs/procfs/sysrq.rs | `write_trigger` | 266 | 24 |  |
| fs/procfs/timer_list.rs | `generate` | 53 | 63 |  |
| fs/procfs/uptime.rs | `account_idle_cycles` | 23 | 8 | yes |
| fs/procfs/uptime.rs | `total_idle_cycles` | 30 | 13 |  |
| fs/procfs/uptime.rs | `generate` | 42 | 25 |  |
| fs/procfs/uptime.rs | `read_time_cycles` | 66 | 6 |  |
| fs/procfs/uptime.rs | `boot_time_cycles` | 71 | 5 |  |
| fs/procfs/version.rs | `generate` | 24 | 14 |  |
| fs/procfs/version.rs | `get_version_string` | 37 | 6 |  |
| fs/procfs/version.rs | `get_release_string` | 42 | 5 |  |
| fs/procfs/vmstat.rs | `zone_name` | 33 | 13 |  |
| fs/procfs/vmstat.rs | `nr_free_pages` | 45 | 94 |  |
| fs/procfs/vmstat.rs | `generate` | 138 | 11 |  |
| fs/procfs/vmstat.rs | `generate_buddyinfo` | 148 | 27 |  |
| fs/pty.rs | `new` | 98 | 9 | yes |
| fs/pty.rs | `len` | 106 | 5 | yes |
| fs/pty.rs | `space` | 110 | 6 |  |
| fs/pty.rs | `write_exact` | 115 | 13 |  |
| fs/pty.rs | `read` | 127 | 40 | yes |
| fs/pty.rs | `new` | 166 | 16 | yes |
| fs/pty.rs | `master_rx_len` | 181 | 5 |  |
| fs/pty.rs | `master_rx_space` | 185 | 5 |  |
| fs/pty.rs | `master_rx_read` | 189 | 5 |  |
| fs/pty.rs | `master_rx_write_exact` | 193 | 9 |  |
| fs/pty.rs | `pty_output` | 201 | 22 |  |
| fs/pty.rs | `alloc_pty` | 222 | 34 |  |
| fs/pty.rs | `destroy_pair` | 255 | 17 |  |
| fs/pty.rs | `devfs_register` | 271 | 8 |  |
| fs/pty.rs | `devfs_add_slave_node` | 278 | 15 |  |
| fs/pty.rs | `devfs_remove_slave_node` | 292 | 16 |  |
| fs/pty.rs | `evict_pts_dentry` | 307 | 12 |  |
| fs/pty.rs | `ptmx_open` | 318 | 14 |  |
| fs/pty.rs | `slave_open` | 331 | 29 |  |
| fs/pty.rs | `pair_of` | 359 | 10 |  |
| fs/pty.rs | `pair_arc` | 368 | 11 |  |
| fs/pty.rs | `take_pair_arc` | 378 | 11 |  |
| fs/pty.rs | `nonblock` | 388 | 5 |  |
| fs/pty.rs | `send_sigpipe` | 392 | 11 |  |
| fs/pty.rs | `ptmx_read` | 402 | 61 |  |
| fs/pty.rs | `ptmx_write` | 462 | 20 |  |
| fs/pty.rs | `ptmx_poll` | 481 | 22 |  |
| fs/pty.rs | `ptmx_close` | 502 | 43 |  |
| fs/pty.rs | `pty_slave_read` | 544 | 12 |  |
| fs/pty.rs | `pty_slave_write` | 555 | 89 |  |
| fs/pty.rs | `pty_slave_poll` | 643 | 24 |  |
| fs/pty.rs | `pty_slave_close` | 666 | 45 |  |
| fs/pty.rs | `copy_out` | 710 | 8 | yes |
| fs/pty.rs | `copy_in` | 717 | 7 | yes |
| fs/pty.rs | `pty_ioctl` | 723 | 266 | yes |
| fs/readahead.rs | `new` | 53 | 13 | yes |
| fs/readahead.rs | `on_read` | 65 | 41 | yes |
| fs/rootfs.rs | `new` | 53 | 31 | yes |
| fs/rootfs.rs | `path_cache_init` | 83 | 29 |  |
| fs/rootfs.rs | `stamp_tree` | 111 | 9 |  |
| fs/rootfs.rs | `path_cache_lookup` | 119 | 22 |  |
| fs/rootfs.rs | `path_cache_add` | 140 | 19 |  |
| fs/rootfs.rs | `path_cache_flush` | 158 | 13 |  |
| fs/rootfs.rs | `rootfs_path_hash` | 170 | 12 |  |
| fs/rootfs.rs | `wall_secs` | 181 | 6 | yes |
| fs/rootfs.rs | `rootfs_count_links` | 186 | 14 |  |
| fs/rootfs.rs | `count_links_in` | 199 | 17 |  |
| fs/rootfs.rs | `rootfs_subtree_contains` | 215 | 15 |  |
| fs/rootfs.rs | `get_rootfs_sb` | 229 | 13 |  |
| fs/rootfs.rs | `get_rootfs_sb_ref` | 241 | 5 |  |
| fs/rootfs.rs | `get_root_mount` | 245 | 71 |  |
| fs/rootfs.rs | `new` | 315 | 22 | yes |
| fs/rootfs.rs | `name` | 336 | 8 | yes |
| fs/rootfs.rs | `new_dir` | 343 | 6 |  |
| fs/rootfs.rs | `new_file` | 348 | 8 |  |
| fs/rootfs.rs | `new_symlink` | 355 | 8 |  |
| fs/rootfs.rs | `get` | 362 | 6 | yes |
| fs/rootfs.rs | `put` | 367 | 8 | yes |
| fs/rootfs.rs | `add_child` | 374 | 12 | yes |
| fs/rootfs.rs | `remove_child` | 385 | 16 | yes |
| fs/rootfs.rs | `set_name` | 400 | 7 |  |
| fs/rootfs.rs | `rename_child` | 406 | 12 |  |
| fs/rootfs.rs | `find_child` | 417 | 13 |  |
| fs/rootfs.rs | `list_children` | 429 | 8 |  |
| fs/rootfs.rs | `is_dir` | 436 | 6 | yes |
| fs/rootfs.rs | `is_file` | 441 | 6 |  |
| fs/rootfs.rs | `is_symlink` | 446 | 6 | yes |
| fs/rootfs.rs | `get_link_target` | 451 | 6 |  |
| fs/rootfs.rs | `read_data` | 456 | 17 | yes |
| fs/rootfs.rs | `write_data` | 472 | 31 | yes |
| fs/rootfs.rs | `new` | 502 | 18 | yes |
| fs/rootfs.rs | `get_root` | 519 | 7 | yes |
| fs/rootfs.rs | `alloc_ino` | 525 | 6 |  |
| fs/rootfs.rs | `create_file` | 530 | 45 | yes |
| fs/rootfs.rs | `create_dir` | 574 | 61 |  |
| fs/rootfs.rs | `link` | 634 | 90 |  |
| fs/rootfs.rs | `lookup` | 723 | 45 | yes |
| fs/rootfs.rs | `lookup_walk` | 767 | 39 |  |
| fs/rootfs.rs | `list_dir` | 805 | 13 | yes |
| fs/rootfs.rs | `mkdir` | 817 | 44 |  |
| fs/rootfs.rs | `unlink` | 860 | 53 |  |
| fs/rootfs.rs | `rmdir` | 912 | 58 |  |
| fs/rootfs.rs | `rename` | 969 | 112 |  |
| fs/rootfs.rs | `symlink` | 1080 | 45 |  |
| fs/rootfs.rs | `readlink` | 1124 | 23 |  |
| fs/rootfs.rs | `follow_link_internal` | 1146 | 41 |  |
| fs/rootfs.rs | `lookup_follow` | 1186 | 60 |  |
| fs/rootfs.rs | `rootfs_mount` | 1245 | 18 |  |
| fs/rootfs.rs | `init_rootfs` | 1262 | 43 |  |
| fs/rootfs.rs | `get_root_node` | 1304 | 9 |  |
| fs/rootfs.rs | `get_rootfs` | 1312 | 7 | yes |
| fs/rootfs.rs | `create_root_inode` | 1318 | 25 | yes |
| fs/rootfs.rs | `rootfs_lookup` | 1342 | 18 |  |
| fs/rootfs.rs | `rootfs_mkdir` | 1359 | 40 |  |
| fs/rootfs.rs | `rootfs_unlink` | 1398 | 42 |  |
| fs/rootfs.rs | `rootfs_rmdir` | 1439 | 46 |  |
| fs/rootfs.rs | `rootfs_create` | 1484 | 39 |  |
| fs/rootfs.rs | `rootfs_symlink` | 1522 | 37 |  |
| fs/rootfs.rs | `rootfs_link` | 1558 | 58 |  |
| fs/rootfs.rs | `rootfs_rename` | 1615 | 63 |  |
| fs/rootfs.rs | `rootfs_readlink` | 1677 | 24 |  |
| fs/rootfs.rs | `rootfs_setattr` | 1700 | 45 |  |
| fs/rootfs.rs | `rootfs_getattr` | 1744 | 34 |  |
| fs/rootfs.rs | `rootfs_file_read` | 1777 | 26 |  |
| fs/rootfs.rs | `rootfs_file_write` | 1802 | 24 |  |
| fs/rootfs.rs | `rootfs_file_lseek` | 1825 | 24 |  |
| fs/rootfs.rs | `rootfs_file_close` | 1848 | 15 |  |
| fs/rootfs.rs | `rootfs_get_file_ops` | 1862 | 13 |  |
| fs/rootfs.rs | `rootfs_readdir` | 1874 | 31 |  |
| fs/rootfs.rs | `rootfs_destroy_inode` | 1904 | 40 |  |
| fs/rootfs.rs | `rootfs_iget` | 1943 | 28 |  |
| fs/rootfs.rs | `rootfs_node_size` | 1970 | 13 |  |
| fs/stat.rs | `new` | 91 | 25 | yes |
| fs/stat.rs | `set_regular_file` | 115 | 6 | yes |
| fs/stat.rs | `set_directory` | 120 | 6 |  |
| fs/stat.rs | `set_char_device` | 125 | 6 |  |
| fs/stat.rs | `set_block_device` | 130 | 6 |  |
| fs/stat.rs | `set_fifo` | 135 | 6 | yes |
| fs/stat.rs | `set_symlink` | 140 | 6 |  |
| fs/stat.rs | `set_socket` | 145 | 6 | yes |
| fs/stat.rs | `is_regular_file` | 150 | 6 | yes |
| fs/stat.rs | `is_directory` | 155 | 6 | yes |
| fs/stat.rs | `is_char_device` | 160 | 6 | yes |
| fs/stat.rs | `is_block_device` | 165 | 6 | yes |
| fs/stat.rs | `is_fifo` | 170 | 6 | yes |
| fs/stat.rs | `is_symlink` | 175 | 6 | yes |
| fs/stat.rs | `is_socket` | 180 | 6 | yes |
| fs/stat.rs | `set_mode` | 185 | 9 |  |
| fs/stat.rs | `get_mode` | 193 | 7 |  |
| fs/stat.rs | `default` | 199 | 39 | yes |
| fs/stat.rs | `from` | 237 | 27 | yes |
| fs/stat.rs | `user_stat_size` | 263 | 11 | yes |
| fs/stat.rs | `copy_stat_to_user` | 273 | 95 | yes |
| fs/stat.rs | `new` | 367 | 30 | yes |
| fs/stat.rs | `default` | 396 | 6 | yes |
| fs/superblock.rs | `new` | 45 | 5 | yes |
| fs/superblock.rs | `is_readonly` | 49 | 5 | yes |
| fs/superblock.rs | `is_active` | 53 | 5 |  |
| fs/superblock.rs | `bits` | 57 | 33 | yes |
| fs/superblock.rs | `new` | 89 | 24 | yes |
| fs/superblock.rs | `set_type` | 112 | 6 | yes |
| fs/superblock.rs | `set_fs_info` | 117 | 6 |  |
| fs/superblock.rs | `set_flags` | 122 | 19 | yes |
| fs/superblock.rs | `new` | 140 | 34 | yes |
| fs/superblock.rs | `new` | 173 | 17 | yes |
| fs/superblock.rs | `mount_fs` | 189 | 19 |  |
| fs/superblock.rs | `kill_super` | 207 | 17 |  |
| fs/superblock.rs | `new` | 223 | 9 | yes |
| fs/superblock.rs | `register` | 231 | 17 | yes |
| fs/superblock.rs | `unregister` | 247 | 19 |  |
| fs/superblock.rs | `get` | 265 | 18 | yes |
| fs/superblock.rs | `register_filesystem` | 282 | 5 | yes |
| fs/superblock.rs | `unregister_filesystem` | 286 | 5 |  |
| fs/superblock.rs | `get_fs_type` | 290 | 5 |  |
| fs/superblock.rs | `do_mount` | 294 | 21 | yes |
| fs/superblock.rs | `do_umount` | 314 | 16 |  |
| fs/sysfs.rs | `subsystem` | 121 | 67 |  |
| fs/sysfs.rs | `new_root` | 187 | 16 |  |
| fs/sysfs.rs | `new` | 202 | 17 | yes |
| fs/sysfs.rs | `add_child` | 218 | 11 | yes |
| fs/sysfs.rs | `remove_child` | 228 | 6 | yes |
| fs/sysfs.rs | `find_child` | 233 | 7 |  |
| fs/sysfs.rs | `set_attr` | 239 | 19 |  |
| fs/sysfs.rs | `find_attr` | 257 | 9 |  |
| fs/sysfs.rs | `attr_content` | 265 | 12 |  |
| fs/sysfs.rs | `path` | 276 | 33 | yes |
| fs/sysfs.rs | `alloc_ino` | 308 | 12 |  |
| fs/sysfs.rs | `sysfs_root` | 319 | 30 |  |
| fs/sysfs.rs | `coldboot_uevent_replay` | 348 | 28 | yes |
| fs/sysfs.rs | `device_add_payload` | 375 | 22 |  |
| fs/sysfs.rs | `lookup_path` | 396 | 14 | yes |
| fs/sysfs.rs | `mk_dir` | 409 | 8 |  |
| fs/sysfs.rs | `mk_link` | 416 | 8 |  |
| fs/sysfs.rs | `attr_ro` | 423 | 9 |  |
| fs/sysfs.rs | `attr_rw` | 431 | 15 |  |
| fs/sysfs.rs | `add_uevent_attr` | 445 | 72 |  |
| fs/sysfs.rs | `parse_uevent_action` | 516 | 24 |  |
| fs/sysfs.rs | `cpu_range_str` | 539 | 10 |  |
| fs/sysfs.rs | `possible_cpus` | 548 | 6 |  |
| fs/sysfs.rs | `add_net_device` | 553 | 77 |  |
| fs/sysfs.rs | `subsystem_link_target` | 629 | 17 |  |
| fs/sysfs.rs | `ensure_devices_chain` | 645 | 53 |  |
| fs/sysfs.rs | `register_block_disk_at` | 697 | 115 |  |
| fs/sysfs.rs | `unregister_block_disk` | 811 | 17 |  |
| fs/sysfs.rs | `find_disk` | 827 | 89 |  |
| fs/sysfs.rs | `dev_env` | 915 | 31 |  |
| fs/sysfs.rs | `add_env_uevent_attr` | 945 | 48 |  |
| fs/sysfs.rs | `register_input_event` | 992 | 110 |  |
| fs/sysfs.rs | `unregister_input_event` | 1101 | 53 |  |
| fs/sysfs.rs | `register_drm_device` | 1153 | 132 |  |
| fs/sysfs.rs | `add_tty_device` | 1284 | 31 |  |
| fs/sysfs.rs | `build_tree` | 1314 | 463 |  |
| fs/sysfs.rs | `started_cpus` | 1776 | 20 |  |
| fs/sysfs.rs | `push_env` | 1795 | 18 |  |
| fs/sysfs.rs | `uevent_send_full` | 1812 | 16 |  |
| fs/sysfs.rs | `uevent_payload_for` | 1827 | 25 |  |
| fs/sysfs.rs | `subsystem_from_devpath` | 1851 | 17 |  |
| fs/sysfs.rs | `uevent_send` | 1867 | 8 |  |
| fs/sysfs.rs | `kobject_uevent` | 1874 | 26 |  |
| fs/sysfs.rs | `netdev_uevent` | 1899 | 8 | yes |
| fs/sysfs.rs | `uevent_seqnum` | 1906 | 32 |  |
| fs/sysfs.rs | `sysfs_mount_cb` | 1937 | 13 |  |
| fs/sysfs.rs | `sysfs_kill_sb` | 1949 | 6 |  |
| fs/sysfs.rs | `get_sysfs_sb` | 1954 | 7 |  |
| fs/sysfs.rs | `init_sysfs` | 1960 | 28 |  |
| fs/sysfs.rs | `mount_sysfs` | 1987 | 23 |  |
| fs/sysfs.rs | `is_mounted` | 2009 | 6 | yes |
| fs/sysfs.rs | `create_root_inode` | 2014 | 53 | yes |
| fs/sysfs.rs | `entry_of` | 2066 | 13 |  |
| fs/sysfs.rs | `resolve_child` | 2078 | 26 |  |
| fs/sysfs.rs | `sysfs_lookup` | 2103 | 11 |  |
| fs/sysfs.rs | `sysfs_iget` | 2113 | 32 |  |
| fs/sysfs.rs | `sysfs_getattr` | 2144 | 48 |  |
| fs/sysfs.rs | `sysfs_setattr` | 2191 | 41 |  |
| fs/sysfs.rs | `sysfs_readlink` | 2231 | 17 |  |
| fs/sysfs.rs | `sysfs_readdir` | 2247 | 37 |  |
| fs/sysfs.rs | `sysfs_open` | 2283 | 25 |  |
| fs/sysfs.rs | `sysfs_get_file_ops` | 2307 | 13 |  |
| fs/sysfs.rs | `sysfs_destroy_inode` | 2319 | 12 |  |
| fs/sysfs.rs | `sysfs_mkdir` | 2330 | 54 |  |
| fs/sysfs.rs | `sysfs_file_read` | 2383 | 20 |  |
| fs/sysfs.rs | `sysfs_file_write` | 2402 | 30 |  |
| fs/sysfs.rs | `sysfs_file_lseek` | 2431 | 22 |  |
| fs/sysfs.rs | `sysfs_file_close` | 2452 | 15 |  |
| fs/sysfs.rs | `sysfs_file_poll` | 2466 | 22 |  |
| fs/sysfs.rs | `mount_sys_instance` | 2487 | 12 |  |
| fs/sysfs.rs | `mount_pstore` | 2498 | 15 |  |
| fs/sysfs.rs | `mount_register` | 2512 | 5 | yes |
| fs/tmpfs.rs | `wall_secs` | 63 | 23 | yes |
| fs/tmpfs.rs | `new` | 85 | 12 | yes |
| fs/tmpfs.rs | `drop` | 96 | 79 | yes |
| fs/tmpfs.rs | `new` | 174 | 37 | yes |
| fs/tmpfs.rs | `name` | 210 | 8 | yes |
| fs/tmpfs.rs | `set_name` | 217 | 6 |  |
| fs/tmpfs.rs | `is_dir` | 222 | 5 | yes |
| fs/tmpfs.rs | `is_file` | 226 | 5 |  |
| fs/tmpfs.rs | `is_symlink` | 230 | 5 | yes |
| fs/tmpfs.rs | `add_child` | 234 | 5 | yes |
| fs/tmpfs.rs | `remove_child` | 238 | 11 | yes |
| fs/tmpfs.rs | `find_child` | 248 | 6 |  |
| fs/tmpfs.rs | `list_children` | 253 | 5 |  |
| fs/tmpfs.rs | `file_size` | 257 | 6 | yes |
| fs/tmpfs.rs | `page_count` | 262 | 20 | yes |
| fs/tmpfs.rs | `read_at` | 281 | 63 | yes |
| fs/tmpfs.rs | `write_at` | 343 | 111 | yes |
| fs/tmpfs.rs | `fallocate_alloc` | 453 | 33 | yes |
| fs/tmpfs.rs | `punch_hole` | 485 | 64 | yes |
| fs/tmpfs.rs | `truncate` | 548 | 73 | yes |
| fs/tmpfs.rs | `reserve_page` | 620 | 51 |  |
| fs/tmpfs.rs | `new` | 670 | 33 | yes |
| fs/tmpfs.rs | `stamp_final_address` | 702 | 9 |  |
| fs/tmpfs.rs | `alloc_ino` | 710 | 6 |  |
| fs/tmpfs.rs | `new_node` | 715 | 28 |  |
| fs/tmpfs.rs | `parse_size_pages` | 742 | 68 |  |
| fs/tmpfs.rs | `create_mount_instance` | 809 | 20 |  |
| fs/tmpfs.rs | `tag_mount_instance_type` | 828 | 9 |  |
| fs/tmpfs.rs | `tmpfs_make_inode` | 836 | 25 |  |
| fs/tmpfs.rs | `tmpfs_mount` | 860 | 18 |  |
| fs/tmpfs.rs | `ensure_registered` | 877 | 14 |  |
| fs/tmpfs.rs | `node_of` | 890 | 11 |  |
| fs/tmpfs.rs | `tmpfs_count_links` | 900 | 2 |  |
| fs/tmpfs.rs | `walk` | 901 | 16 | yes |
| fs/tmpfs.rs | `tmpfs_subtree_contains` | 916 | 16 |  |
| fs/tmpfs.rs | `tmpfs_lookup` | 931 | 13 |  |
| fs/tmpfs.rs | `tmpfs_create` | 943 | 21 |  |
| fs/tmpfs.rs | `tmpfs_mkdir` | 963 | 20 |  |
| fs/tmpfs.rs | `tmpfs_symlink` | 982 | 24 |  |
| fs/tmpfs.rs | `tmpfs_mknod` | 1005 | 44 |  |
| fs/tmpfs.rs | `tmpfs_link` | 1048 | 44 |  |
| fs/tmpfs.rs | `tmpfs_unlink` | 1091 | 25 |  |
| fs/tmpfs.rs | `tmpfs_rmdir` | 1115 | 28 |  |
| fs/tmpfs.rs | `tmpfs_rename` | 1142 | 47 |  |
| fs/tmpfs.rs | `tmpfs_readlink` | 1188 | 20 |  |
| fs/tmpfs.rs | `tmpfs_getattr` | 1207 | 35 |  |
| fs/tmpfs.rs | `tmpfs_setattr` | 1241 | 71 |  |
| fs/tmpfs.rs | `tmpfs_readdir` | 1311 | 41 |  |
| fs/tmpfs.rs | `tmpfs_iget` | 1351 | 14 |  |
| fs/tmpfs.rs | `tmpfs_destroy_inode` | 1364 | 21 |  |
| fs/tmpfs.rs | `tmpfs_file_read` | 1384 | 54 |  |
| fs/tmpfs.rs | `tmpfs_file_write` | 1437 | 52 |  |
| fs/tmpfs.rs | `tmpfs_file_lseek` | 1488 | 23 |  |
| fs/tmpfs.rs | `tmpfs_file_close` | 1510 | 20 |  |
| fs/tmpfs.rs | `is_tmpfs_file` | 1529 | 14 |  |
| fs/tmpfs.rs | `tmpfs_file_read_at` | 1542 | 26 |  |
| fs/tmpfs.rs | `tmpfs_get_file_ops` | 1567 | 165 |  |
| fs/tmpfs.rs | `mount_tmpfs` | 1731 | 13 |  |
| fs/tmpfs.rs | `mount_ramfs` | 1743 | 11 |  |
| fs/tmpfs.rs | `mount_devpts` | 1753 | 13 |  |
| fs/tmpfs.rs | `mount_securityfs` | 1765 | 12 |  |
| fs/tmpfs.rs | `mount_mqueue` | 1776 | 12 |  |
| fs/tmpfs.rs | `mount_configfs` | 1787 | 32 |  |
| fs/tmpfs.rs | `mount_register` | 1818 | 9 | yes |
| fs/tracefs.rs | `next_ino` | 50 | 6 |  |
| fs/tracefs.rs | `tracefs_root` | 55 | 13 |  |
| fs/tracefs.rs | `create_dir` | 67 | 43 |  |
| fs/tracefs.rs | `create_file` | 109 | 16 | yes |
| fs/tracefs.rs | `create_file_inner` | 124 | 69 |  |
| fs/tracefs.rs | `create_stream_file` | 192 | 25 |  |
| fs/tracefs.rs | `tracefs_mount_cb` | 216 | 14 |  |
| fs/tracefs.rs | `tracefs_kill_sb` | 229 | 12 |  |
| fs/tracefs.rs | `init_tracefs` | 240 | 36 |  |
| fs/tracefs.rs | `mount_at` | 275 | 16 |  |
| fs/tracefs.rs | `create_root_inode` | 290 | 46 | yes |
| fs/tracefs.rs | `entry_of` | 335 | 6 |  |
| fs/tracefs.rs | `resolve_child` | 340 | 11 |  |
| fs/tracefs.rs | `tracefs_lookup` | 350 | 11 |  |
| fs/tracefs.rs | `tracefs_iget` | 360 | 31 |  |
| fs/tracefs.rs | `tracefs_getattr` | 390 | 52 |  |
| fs/tracefs.rs | `tracefs_setattr` | 441 | 49 |  |
| fs/tracefs.rs | `tracefs_readdir` | 489 | 28 |  |
| fs/tracefs.rs | `tracefs_open` | 516 | 31 |  |
| fs/tracefs.rs | `tracefs_get_file_ops` | 546 | 13 |  |
| fs/tracefs.rs | `tracefs_destroy_inode` | 558 | 47 |  |
| fs/tracefs.rs | `tracefs_file_read` | 604 | 36 |  |
| fs/tracefs.rs | `tracefs_file_write` | 639 | 26 |  |
| fs/tracefs.rs | `tracefs_file_lseek` | 664 | 22 |  |
| fs/tracefs.rs | `tracefs_file_close` | 685 | 23 |  |
| fs/tracefs.rs | `mount_tracefs` | 707 | 12 |  |
| fs/tracefs.rs | `mount_register` | 718 | 4 | yes |
| fs/tty.rs | `default` | 132 | 27 | yes |
| fs/tty.rs | `canonical` | 158 | 16 |  |
| fs/tty.rs | `termios_to_kernel_bytes` | 173 | 13 | yes |
| fs/tty.rs | `termios_from_kernel_bytes` | 185 | 19 | yes |
| fs/tty.rs | `termios_to_user_bytes` | 203 | 16 |  |
| fs/tty.rs | `termios_from_user_bytes` | 218 | 19 |  |
| fs/tty.rs | `termios_to_termio_bytes` | 236 | 15 | yes |
| fs/tty.rs | `termios_from_termio_bytes` | 250 | 28 | yes |
| fs/tty.rs | `default` | 277 | 11 | yes |
| fs/tty.rs | `to_le_bytes` | 287 | 10 | yes |
| fs/tty.rs | `from_le_bytes` | 296 | 37 | yes |
| fs/tty.rs | `new` | 332 | 12 | yes |
| fs/tty.rs | `push` | 343 | 11 | yes |
| fs/tty.rs | `uncommitted_len` | 353 | 6 |  |
| fs/tty.rs | `erase_last` | 358 | 11 |  |
| fs/tty.rs | `kill_line` | 368 | 6 |  |
| fs/tty.rs | `commit` | 373 | 5 | yes |
| fs/tty.rs | `set_eof` | 377 | 6 |  |
| fs/tty.rs | `readable_len` | 382 | 7 |  |
| fs/tty.rs | `read` | 388 | 32 | yes |
| fs/tty.rs | `isig_char` | 419 | 29 |  |
| fs/tty.rs | `new` | 447 | 16 | yes |
| fs/tty.rs | `set_output_ctx` | 462 | 8 |  |
| fs/tty.rs | `get_termios` | 469 | 6 | yes |
| fs/tty.rs | `set_termios` | 474 | 6 | yes |
| fs/tty.rs | `lflag` | 479 | 5 |  |
| fs/tty.rs | `echo_enabled` | 483 | 5 |  |
| fs/tty.rs | `get_winsize` | 487 | 6 | yes |
| fs/tty.rs | `set_winsize` | 492 | 9 | yes |
| fs/tty.rs | `send_sigwinch` | 500 | 13 | yes |
| fs/tty.rs | `receive_byte` | 512 | 123 |  |
| fs/tty.rs | `send_isig` | 634 | 14 |  |
| fs/tty.rs | `fionread_count` | 647 | 7 |  |
| fs/tty.rs | `input_poll_ready` | 653 | 8 |  |
| fs/tty.rs | `read_input` | 660 | 76 |  |
| fs/tty.rs | `flush_input` | 735 | 12 | yes |
| fs/tty.rs | `output_translates_nl` | 746 | 7 |  |
| fs/tty.rs | `write_output_raw` | 752 | 21 |  |
| fs/tty.rs | `console_output` | 772 | 18 |  |
| fs/tty.rs | `set_console_sink_quiet` | 789 | 12 |  |
| fs/tty.rs | `console` | 800 | 20 | yes |
| fs/tty.rs | `file_is_tty` | 819 | 17 | yes |
| fs/unsafe_edges.rs | `privdata_ref` | 35 | 19 |  |
| fs/unsafe_edges.rs | `file_privdata_ref` | 53 | 12 |  |
| fs/unsafe_edges.rs | `file_privdata_take` | 64 | 20 |  |
| fs/unsafe_edges.rs | `static_ref` | 83 | 14 | yes |
| fs/unsafe_edges.rs | `static_ref_unchecked` | 96 | 9 | yes |
| fs/unsafe_edges.rs | `file_privdata_arc_ref` | 104 | 16 |  |
| fs/unsafe_edges.rs | `file_privdata_arc_take` | 119 | 18 |  |
| fs/unsafe_edges.rs | `file_privdata_ref_mut` | 136 | 24 |  |
| fs/unsafe_edges.rs | `file_inode` | 159 | 12 | yes |
| fs/unsafe_edges.rs | `file_inode_install` | 170 | 11 | yes |
| fs/unsafe_edges.rs | `file_privdata_take_arc` | 180 | 19 |  |
| fs/unsafe_edges.rs | `privdata_ref_leaked` | 198 | 10 |  |
| fs/unsafe_edges.rs | `file_inode_arc` | 207 | 13 | yes |
| fs/unsafe_edges.rs | `file_ra_state_mut` | 219 | 11 | yes |
| fs/unsafe_edges.rs | `as_deref_ref` | 229 | 4 |  |
| fs/unsafe_edges.rs | `as_deref_ref` | 232 | 11 |  |
| fs/unsafe_edges.rs | `privdata_take_box` | 242 | 15 |  |
| fs/unsafe_edges.rs | `privdata_drop_arc` | 256 | 11 |  |
| fs/unsafe_edges.rs | `kill_sb_reclaim_box` | 266 | 13 |  |
| fs/unsafe_edges.rs | `file_privdata_arc_clone` | 278 | 17 |  |
| fs/unsafe_edges.rs | `vec_zeroed_sized` | 294 | 14 | yes |
| fs/unsafe_edges.rs | `kernel_slice` | 307 | 7 | yes |
| fs/unsafe_edges.rs | `kernel_slice_mut` | 313 | 8 | yes |
| fs/unsafe_edges.rs | `file_close` | 320 | 11 | yes |
| fs/unsafe_edges.rs | `file_privdata_arc_static` | 330 | 10 |  |
| fs/vfs.rs | `new` | 110 | 10 | yes |
| fs/vfs.rs | `with_inode` | 119 | 10 | yes |
| fs/vfs.rs | `is_valid` | 128 | 7 | yes |
| fs/vfs.rs | `default` | 134 | 59 | yes |
| fs/vfs.rs | `vfs_root_publish` | 192 | 21 | yes |
| fs/vfs.rs | `get_vfs_root` | 212 | 30 | yes |
| fs/vfs.rs | `follow_mount` | 241 | 19 | yes |
| fs/vfs.rs | `mount_flags_for_path` | 259 | 34 | yes |
| fs/vfs.rs | `path_on_readonly_mount` | 292 | 15 | yes |
| fs/vfs.rs | `file_on_readonly_mount` | 306 | 11 | yes |
| fs/vfs.rs | `dentry_on_readonly_mount` | 316 | 24 | yes |
| fs/vfs.rs | `check_not_readonly` | 339 | 21 | yes |
| fs/vfs.rs | `ns_may_reflag` | 359 | 13 | yes |
| fs/vfs.rs | `apply_mnt_flags` | 371 | 58 | yes |
| fs/vfs.rs | `vfs_remount_dentry` | 428 | 42 | yes |
| fs/vfs.rs | `vfs_remount` | 469 | 67 | yes |
| fs/vfs.rs | `shallow_dentry_walk` | 535 | 20 | yes |
| fs/vfs.rs | `vfs_mount` | 554 | 139 | yes |
| fs/vfs.rs | `dentry_in_mount_subtree` | 692 | 33 | yes |
| fs/vfs.rs | `mount_is_busy` | 724 | 62 | yes |
| fs/vfs.rs | `sync_dirty_filesystems` | 785 | 47 | yes |
| fs/vfs.rs | `vfs_umount` | 831 | 87 | yes |
| fs/vfs.rs | `vfs_umount_object` | 917 | 85 | yes |
| fs/vfs.rs | `dentry_relpath_below` | 1001 | 46 | yes |
| fs/vfs.rs | `mountpoint_chain` | 1046 | 16 | yes |
| fs/vfs.rs | `mount_index_insert` | 1061 | 19 | yes |
| fs/vfs.rs | `mount_index_remove` | 1079 | 17 | yes |
| fs/vfs.rs | `mount_index_rebuild` | 1095 | 33 | yes |
| fs/vfs.rs | `collect_subtree_mounts` | 1127 | 101 | yes |
| fs/vfs.rs | `vfs_bind_mount` | 1227 | 279 | yes |
| fs/vfs.rs | `bind_clone_attach` | 1505 | 229 | yes |
| fs/vfs.rs | `vfs_virtual_switch_root` | 1733 | 84 | yes |
| fs/vfs.rs | `vfs_move_mount` | 1816 | 171 | yes |
| fs/vfs.rs | `vfs_attach_mount_at` | 1986 | 77 | yes |
| fs/vfs.rs | `vfs_covering_mount` | 2062 | 33 | yes |
| fs/vfs.rs | `vfs_detach_mount` | 2094 | 35 | yes |
| fs/vfs.rs | `vfs_governing_mount_of` | 2128 | 50 | yes |
| fs/vfs.rs | `vfs_clone_submounts` | 2177 | 15 | yes |
| fs/vfs.rs | `vfs_collect_submounts` | 2191 | 10 | yes |
| fs/vfs.rs | `vfs_mount_registry_row` | 2200 | 26 | yes |
| fs/vfs.rs | `from_str` | 2225 | 17 | yes |
| fs/vfs.rs | `init` | 2241 | 28 | yes |
| fs/vfs.rs | `get_cwd` | 2268 | 19 | yes |
| fs/vfs.rs | `make_absolute` | 2286 | 21 | yes |
| fs/vfs.rs | `get_process_root` | 2306 | 19 | yes |
| fs/vfs.rs | `chrooted` | 2324 | 10 | yes |
| fs/vfs.rs | `process_rooted` | 2333 | 13 | yes |
| fs/vfs.rs | `path_is_normalized_fast` | 2345 | 45 | yes |
| fs/vfs.rs | `get_cwd_user` | 2389 | 42 | yes |
| fs/vfs.rs | `dentry_user_path` | 2430 | 19 | yes |
| fs/vfs.rs | `make_absolute_rooted` | 2448 | 15 | yes |
| fs/vfs.rs | `is_under_root` | 2462 | 14 | yes |
| fs/vfs.rs | `normalize_with_root` | 2475 | 39 | yes |
| fs/vfs.rs | `dentry_walk_no_symlink` | 2513 | 42 | yes |
| fs/vfs.rs | `sbx_want` | 2554 | 21 | yes |
| fs/vfs.rs | `sbx_noise` | 2574 | 5 | yes |
| fs/vfs.rs | `sbx_trace` | 2578 | 50 | yes |
| fs/vfs.rs | `sbx_dentry_state` | 2627 | 9 | yes |
| fs/vfs.rs | `path_lookup` | 2635 | 792 | yes |
| fs/vfs.rs | `follow_symlink` | 3426 | 18 | yes |
| fs/vfs.rs | `follow_symlink_hinted` | 3443 | 271 | yes |
| fs/vfs.rs | `path_parent_and_name` | 3713 | 34 | yes |
| fs/vfs.rs | `check_parent_write_permission` | 3746 | 23 | yes |
| fs/vfs.rs | `check_parent_exec_permission` | 3768 | 27 | yes |
| fs/vfs.rs | `check_sticky` | 3794 | 49 | yes |
| fs/vfs.rs | `dentry_parent_inode` | 3842 | 7 | yes |
| fs/vfs.rs | `notify_about` | 3848 | 14 | yes |
| fs/vfs.rs | `inode_nlink` | 3861 | 12 | yes |
| fs/vfs.rs | `lookup_parent_dir` | 3872 | 41 | yes |
| fs/vfs.rs | `vfs_dir_epoch` | 3912 | 7 | yes |
| fs/vfs.rs | `vfs_dir_epoch_bump` | 3918 | 11 | yes |
| fs/vfs.rs | `vfs_mkdir` | 3928 | 172 | yes |
| fs/vfs.rs | `vfs_mknod` | 4099 | 63 | yes |
| fs/vfs.rs | `vfs_symlink` | 4161 | 62 | yes |
| fs/vfs.rs | `vfs_rmdir` | 4222 | 164 | yes |
| fs/vfs.rs | `vfs_unlink` | 4385 | 142 | yes |
| fs/vfs.rs | `vfs_link` | 4526 | 75 | yes |
| fs/vfs.rs | `vfs_rename` | 4600 | 16 | yes |
| fs/vfs.rs | `vfs_rename_locked` | 4615 | 194 | yes |
| fs/vfs.rs | `is_ancestor_of` | 4808 | 50 | yes |
| fs/vfs.rs | `vfs_rename_exchange` | 4857 | 67 | yes |
| fs/vfs.rs | `vfs_chmod` | 4923 | 63 | yes |
| fs/vfs.rs | `vfs_chown` | 4985 | 105 | yes |
| fs/vfs.rs | `vfs_truncate` | 5089 | 67 | yes |
| fs/vfs.rs | `vfs_ftruncate` | 5155 | 67 | yes |
| fs/vfs.rs | `vfs_stat` | 5221 | 49 | yes |
| fs/vfs.rs | `in_group_or_capable_dir` | 5269 | 20 | yes |
| fs/vfs.rs | `mode_strip_sgid` | 5288 | 21 | yes |
| fs/vfs.rs | `prepare_create_mode` | 5308 | 18 | yes |
| fs/vfs.rs | `inherit_setgid_dir_gid` | 5325 | 49 | yes |
| fs/vfs.rs | `file_open_tmpfile` | 5373 | 55 | yes |
| fs/vfs.rs | `file_open` | 5427 | 419 | yes |
| fs/vfs.rs | `open_file_arc_nofd` | 5845 | 19 | yes |
| fs/vfs.rs | `open_file_arc_from_vpath` | 5863 | 32 | yes |
| fs/vfs.rs | `file_close` | 5894 | 15 | yes |
| fs/vfs.rs | `file_read` | 5908 | 28 | yes |
| fs/vfs.rs | `file_write` | 5935 | 20 | yes |
| fs/vfs.rs | `file_stat` | 5954 | 60 | yes |
| fs/vfs.rs | `stat_file_by_path` | 6013 | 6 | yes |
| fs/vfs.rs | `stat_file_by_path_with_flags` | 6018 | 102 | yes |
| fs/vfs.rs | `resolve_lock_region` | 6119 | 71 | yes |
| fs/vfs.rs | `file_fcntl` | 6189 | 581 | yes |
| fs/vfs.rs | `file_mkdir` | 6769 | 14 | yes |
| fs/vfs.rs | `file_rmdir` | 6782 | 14 | yes |
| fs/vfs.rs | `file_unlink` | 6795 | 15 | yes |
| fs/vfs.rs | `file_link` | 6809 | 17 | yes |
| fs/vfs.rs | `file_opendir` | 6825 | 68 | yes |
| fs/vfs.rs | `getdents_with_dots` | 6892 | 35 | yes |
| fs/vfs.rs | `file_getdents64` | 6926 | 195 | yes |
| fs/vfs.rs | `mem_file_content` | 7120 | 8 | yes |
| fs/vfs.rs | `mem_file_content_mut` | 7127 | 9 | yes |
| fs/vfs.rs | `mem_file_take_owned` | 7135 | 8 | yes |
| fs/vfs.rs | `mem_file_read` | 7142 | 20 | yes |
| fs/vfs.rs | `mem_file_stat` | 7161 | 29 | yes |
| fs/vfs.rs | `mem_file_lseek` | 7189 | 32 | yes |
| fs/vfs.rs | `mem_file_close` | 7220 | 16 | yes |
| fs/vfs.rs | `mem_file_poll` | 7235 | 37 | yes |
| fs/vfs.rs | `vfs_fallocate` | 7271 | 85 | yes |
| fs/vfs.rs | `vfs_utimensat` | 7355 | 10 | yes |
| fs/vfs.rs | `vfs_utimensat_ex` | 7364 | 85 | yes |
| fs/vfs.rs | `procfs_dir_readdir` | 7448 | 19 | yes |
| fs/vfs.rs | `open_procfs_dir` | 7466 | 17 | yes |
| fs/vfs.rs | `open_mem_file` | 7482 | 9 | yes |
| fs/vfs.rs | `open_mem_file_regen` | 7490 | 27 | yes |
| fs/vfs.rs | `pipe_fcntl` | 7516 | 13 | yes |
| fs/vt.rs | `is_vt_devno` | 123 | 7 |  |
| fs/vt.rs | `vt_opened` | 129 | 6 |  |
| fs/vt.rs | `vt_closed` | 134 | 6 |  |
| fs/vt.rs | `active_vt` | 139 | 6 |  |
| fs/vt.rs | `wake_switch_waiters` | 144 | 8 |  |
| fs/vt.rs | `wait_switch_round` | 151 | 25 |  |
| fs/vt.rs | `vtdev_read` | 175 | 5 |  |
| fs/vt.rs | `vtdev_write` | 179 | 5 |  |
| fs/vt.rs | `vtdev_poll` | 183 | 9 |  |
| fs/vt.rs | `vtdev_close` | 191 | 27 |  |
| fs/vt.rs | `vt_file_ioctl` | 217 | 224 | yes |
| fs/xattr.rs | `check_name` | 68 | 29 |  |
| fs/xattr.rs | `may_set` | 96 | 66 |  |
| fs/xattr.rs | `set` | 161 | 68 | yes |
| fs/xattr.rs | `get` | 228 | 43 | yes |
| fs/xattr.rs | `list` | 270 | 34 | yes |
| fs/xattr.rs | `remove` | 303 | 18 | yes |
