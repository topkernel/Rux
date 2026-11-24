# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# fs — hot 25787 lines (stay) / cold 3162 lines (leave)

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
| fs/dev_t.rs | `to_u64` | 42 | 10 | yes |
| fs/dev_t.rs | `to_user_dev` | 51 | 12 | yes |
| fs/dev_t.rs | `from_user_dev` | 62 | 10 | yes |
| fs/dev_t.rs | `default` | 71 | 82 | yes |
| fs/elf.rs | `from_bytes` | 348 | 48 | yes |
| fs/elf.rs | `is_executable` | 395 | 7 | yes |
| fs/elf.rs | `is_dynamic` | 401 | 6 |  |
| fs/elf.rs | `check_machine` | 406 | 17 |  |
| fs/elf.rs | `get_program_headers` | 422 | 12 | yes |
| fs/elf.rs | `get_program_header` | 433 | 21 |  |
| fs/elf.rs | `is_load` | 453 | 6 | yes |
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
| fs/ext4_hooks.rs | `mounted` | 184 | 4 | yes |
| fs/ext4_hooks.rs | `unmount` | 187 | 4 |  |
| fs/ext4_hooks.rs | `mount_instance` | 190 | 4 |  |
| fs/ext4_hooks.rs | `loop_invalidate` | 193 | 4 |  |
| fs/ext4_hooks.rs | `boot_read_file` | 196 | 4 |  |
| fs/ext4_hooks.rs | `fsctx_mount_instance` | 199 | 4 |  |
| fs/ext4_hooks.rs | `mount_rows_register` | 202 | 4 |  |
| fs/ext4_hooks.rs | `sync_flushed_data` | 205 | 4 | yes |
| fs/ext4_hooks.rs | `sync_inode_tables` | 208 | 4 | yes |
| fs/ext4_hooks.rs | `shutdown_sync` | 211 | 4 | yes |
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
| fs/file_hooks.rs | `oasync_unregister` | 32 | 7 | yes |
| fs/file_hooks.rs | `char_dev_stat` | 38 | 8 | yes |
| fs/file_hooks.rs | `fifo_forget` | 45 | 6 | yes |
| fs/file_hooks.rs | `fifo_open` | 50 | 10 | yes |
| fs/file_hooks.rs | `memfd_add_seals` | 59 | 4 | yes |
| fs/file_hooks.rs | `memfd_get_seals` | 62 | 4 | yes |
| fs/fs_hooks.rs | `fs_register_hooks` | 93 | 6 | yes |
| fs/fs_hooks.rs | `hooks` | 98 | 5 |  |
| fs/fs_hooks.rs | `notify` | 102 | 4 | yes |
| fs/fs_hooks.rs | `notify_file` | 105 | 4 | yes |
| fs/fs_hooks.rs | `notify_file_close` | 108 | 4 | yes |
| fs/fs_hooks.rs | `alloc_cookie` | 111 | 6 | yes |
| fs/fs_hooks.rs | `is_pipe_ops` | 116 | 2 |  |
| fs/fs_hooks.rs | `is_fifo_ops` | 117 | 2 |  |
| fs/fs_hooks.rs | `is_inotify_ops` | 118 | 2 |  |
| fs/fs_hooks.rs | `is_memfd_ops` | 119 | 2 |  |
| fs/fs_hooks.rs | `pipe_diag_snapshot` | 120 | 2 |  |
| fs/fs_hooks.rs | `memfd_name_by_ino` | 121 | 2 | yes |
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
| fs/fs_struct.rs | `copy_fs_from` | 217 | 17 | yes |
| fs/fs_struct.rs | `get_umask` | 233 | 6 | yes |
| fs/fs_struct.rs | `set_umask` | 238 | 6 | yes |
| fs/fs_struct.rs | `apply_umask` | 243 | 7 |  |
| fs/fs_struct.rs | `default` | 249 | 8 | yes |
| fs/fsctx.rs | `fs_type_known` | 112 | 32 | yes |
| fs/fsctx.rs | `param_key_known` | 143 | 80 | yes |
| fs/fsctx.rs | `new` | 222 | 16 | yes |
| fs/fsctx.rs | `new_reconfigure` | 237 | 18 | yes |
| fs/fsctx.rs | `data_string` | 254 | 40 | yes |
| fs/fsctx.rs | `create_fs_root` | 293 | 99 | yes |
| fs/fsctx.rs | `fsctx_close` | 391 | 16 |  |
| fs/fsctx.rs | `fsctx_of` | 406 | 13 | yes |
| fs/fsctx.rs | `fsctx_of_mut` | 418 | 18 | yes |
| fs/fsctx.rs | `mountfd_close` | 435 | 16 |  |
| fs/fsctx.rs | `mountfd_of` | 450 | 18 | yes |
| fs/fsctx.rs | `treepath_dentry` | 467 | 12 | yes |
| fs/fsctx.rs | `make_detached_mount` | 478 | 35 | yes |
| fs/fsctx.rs | `install_fd_with` | 512 | 25 | yes |
| fs/fsctx.rs | `mount_attr_apply` | 536 | 45 | yes |
| fs/fsctx.rs | `validate_mount_attr` | 580 | 28 | yes |
| fs/fsctx.rs | `validate_mount_attr_pair` | 607 | 31 | yes |
| fs/fsctx.rs | `attr_flags_to_mnt` | 637 | 4 | yes |
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
| fs/leaf_glue.rs | `is_ops` | 37 | 8 |  |
| fs/leaf_glue.rs | `inode_fs_id_is` | 44 | 10 |  |
| fs/leaf_glue.rs | `create_dir` | 53 | 5 | yes |
| fs/leaf_glue.rs | `create_file` | 57 | 16 | yes |
| fs/leaf_glue.rs | `init_debugfs` | 72 | 5 | yes |
| fs/leaf_glue.rs | `debugfs_root_present` | 76 | 5 |  |
| fs/leaf_glue.rs | `create_root_inode` | 80 | 5 | yes |
| fs/leaf_glue.rs | `mount_debugfs` | 84 | 11 | yes |
| fs/leaf_glue.rs | `create_dir` | 94 | 5 | yes |
| fs/leaf_glue.rs | `create_file` | 98 | 16 | yes |
| fs/leaf_glue.rs | `create_stream_file` | 113 | 8 | yes |
| fs/leaf_glue.rs | `tracefs_root_present` | 120 | 5 |  |
| fs/leaf_glue.rs | `init_tracefs` | 124 | 5 | yes |
| fs/leaf_glue.rs | `create_root_inode` | 128 | 5 | yes |
| fs/leaf_glue.rs | `mount_at` | 132 | 11 | yes |
| fs/leaf_glue.rs | `init_cgroupfs` | 142 | 5 |  |
| fs/leaf_glue.rs | `create_root_inode` | 146 | 5 | yes |
| fs/leaf_glue.rs | `mount_cgroupfs` | 150 | 11 |  |
| fs/leaf_glue.rs | `init_sysfs` | 160 | 5 | yes |
| fs/leaf_glue.rs | `mount_sysfs` | 164 | 5 | yes |
| fs/leaf_glue.rs | `create_root_inode` | 168 | 5 | yes |
| fs/leaf_glue.rs | `publish_module` | 172 | 5 | yes |
| fs/leaf_glue.rs | `unpublish_module` | 176 | 5 | yes |
| fs/leaf_glue.rs | `register_drm_device` | 180 | 5 | yes |
| fs/leaf_glue.rs | `netdev_uevent` | 184 | 5 | yes |
| fs/leaf_glue.rs | `register_block_disk_at` | 188 | 11 | yes |
| fs/leaf_glue.rs | `register_input_event` | 198 | 11 | yes |
| fs/leaf_glue.rs | `init_std_fds_for_task` | 208 | 7 | yes |
| fs/leaf_glue.rs | `char_dev_file_stat` | 214 | 14 | yes |
| fs/leaf_glue.rs | `console_lflag` | 227 | 5 | yes |
| fs/leaf_glue.rs | `console_echo_enabled` | 231 | 5 | yes |
| fs/leaf_glue.rs | `console_receive_byte` | 235 | 5 | yes |
| fs/leaf_glue.rs | `console_fg_pgid` | 239 | 5 | yes |
| fs/leaf_glue.rs | `set_console_sink_quiet` | 243 | 11 | yes |
| fs/leaf_glue.rs | `init_rootfs` | 253 | 5 | yes |
| fs/leaf_glue.rs | `create_root_inode` | 257 | 7 | yes |
| fs/leaf_glue.rs | `sb_view` | 263 | 6 |  |
| fs/leaf_glue.rs | `read_file` | 268 | 6 | yes |
| fs/leaf_glue.rs | `lookup_kind` | 273 | 6 |  |
| fs/leaf_glue.rs | `seed_file` | 278 | 13 |  |
| fs/leaf_glue.rs | `init` | 290 | 6 | yes |
| fs/leaf_glue.rs | `create_root_inode` | 295 | 5 | yes |
| fs/leaf_glue.rs | `mkdir` | 299 | 5 | yes |
| fs/leaf_glue.rs | `mknod` | 303 | 5 | yes |
| fs/leaf_glue.rs | `register_char_device` | 307 | 12 | yes |
| fs/leaf_glue.rs | `reg_current_size` | 318 | 12 |  |
| fs/leaf_glue.rs | `shared_fault_file_page` | 329 | 9 | yes |
| fs/leaf_glue.rs | `condev_read` | 337 | 5 | yes |
| fs/leaf_glue.rs | `condev_write` | 341 | 5 | yes |
| fs/leaf_glue.rs | `condev_poll` | 345 | 7 | yes |
| fs/leaf_glue.rs | `is_loop_ops` | 351 | 6 |  |
| fs/leaf_glue.rs | `condev_ops` | 356 | 13 |  |
| fs/leaf_glue.rs | `init_procfs` | 368 | 5 | yes |
| fs/leaf_glue.rs | `mount_procfs` | 372 | 6 | yes |
| fs/leaf_glue.rs | `sb_present` | 377 | 5 | yes |
| fs/leaf_glue.rs | `create_root_inode` | 381 | 7 | yes |
| fs/leaf_glue.rs | `is_valid_pid` | 387 | 6 | yes |
| fs/leaf_glue.rs | `get_fd_file` | 392 | 8 | yes |
| fs/leaf_glue.rs | `is_piddir_ops` | 399 | 6 |  |
| fs/leaf_glue.rs | `fd_dir_ops` | 404 | 6 |  |
| fs/leaf_glue.rs | `core_pattern` | 409 | 11 | yes |
| fs/leaf_glue.rs | `ensure_registered` | 419 | 5 | yes |
| fs/leaf_glue.rs | `create_mount_instance` | 423 | 12 | yes |
| fs/leaf_glue.rs | `is_tmpfs_file` | 434 | 6 | yes |
| fs/leaf_glue.rs | `fallocate` | 439 | 28 | yes |
| fs/leaf_glue.rs | `rfs_mkdir` | 466 | 3 | yes |
| fs/leaf_glue.rs | `rfs_create_dir` | 468 | 2 | yes |
| fs/leaf_glue.rs | `rfs_symlink` | 469 | 2 | yes |
| fs/leaf_glue.rs | `rfs_link` | 470 | 2 | yes |
| fs/leaf_glue.rs | `rfs_create_file` | 471 | 3 | yes |
| fs/leaf_glue.rs | `rfs_remove_child` | 473 | 3 | yes |
| fs/leaf_glue.rs | `rfs_set_node_meta` | 475 | 3 | yes |
| fs/leaf_glue.rs | `rfs_lookup_kind` | 477 | 3 | yes |
| fs/leaf_glue.rs | `rfs_seed_file` | 479 | 36 | yes |
| fs/leaf_glue.rs | `dev_env` | 514 | 32 | yes |
| fs/leaf_hooks.rs | `empty_root` | 156 | 5 |  |
| fs/leaf_hooks.rs | `empty_root_2` | 160 | 5 |  |
| fs/leaf_hooks.rs | `absent` | 164 | 190 |  |
| fs/leaf_hooks.rs | `register_boot_hooks` | 353 | 4 | yes |
| fs/leaf_hooks.rs | `register_populate_hooks` | 356 | 4 | yes |
| fs/leaf_hooks.rs | `register_path_hooks` | 359 | 6 | yes |
| fs/leaf_hooks.rs | `boot_hooks` | 364 | 6 |  |
| fs/leaf_hooks.rs | `populate_hooks` | 369 | 6 |  |
| fs/leaf_hooks.rs | `path_hooks` | 374 | 4 |  |
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
| fs/locks.rs | `snapshot_locks` | 75 | 75 | yes |
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
| fs/mod.rs | `read_file_from_rootfs` | 78 | 5 | yes |
| fs/mount.rs | `register_mount` | 41 | 21 | yes |
| fs/mount.rs | `mounted_as` | 61 | 10 | yes |
| fs/mount.rs | `get_global_mounts` | 70 | 15 |  |
| fs/mount.rs | `get_mounts` | 84 | 12 | yes |
| fs/mount.rs | `get_mount_entries` | 95 | 25 | yes |
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
| fs/mount_table.rs | `lookup` | 48 | 17 | yes |
| fs/mount_table.rs | `register_builtin_handlers` | 64 | 14 |  |
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
| fs/perf_ctr.rs | `timer_inc` | 32 | 8 | yes |
| fs/perf_ctr.rs | `soft_inc` | 39 | 8 | yes |
| fs/perf_ctr.rs | `timer_count` | 46 | 10 | yes |
| fs/perf_ctr.rs | `soft_count` | 55 | 34 | yes |
| fs/perf_ctr.rs | `new` | 88 | 30 | yes |
| fs/perf_ctr.rs | `charge_tick` | 117 | 25 | yes |
| fs/perf_ctr.rs | `note_switch` | 141 | 23 | yes |
| fs/perf_ctr.rs | `note_fork` | 163 | 55 | yes |
| fs/perf_ctr.rs | `update_load_avg` | 217 | 50 | yes |
| fs/perf_ctr.rs | `note_yield` | 266 | 8 | yes |
| fs/perf_ctr.rs | `note_sched_run` | 273 | 18 | yes |
| fs/perf_ctr.rs | `account_idle_cycles` | 290 | 8 | yes |
| fs/perf_ctr.rs | `total_idle_cycles` | 297 | 8 | yes |
| fs/permission.rs | `generic_permission` | 22 | 82 | yes |
| fs/permission.rs | `inode_permission` | 103 | 18 | yes |
| fs/pidfd.rs | `exit_notify_all_waiters` | 54 | 16 |  |
| fs/pidfd.rs | `pin_leader` | 69 | 25 |  |
| fs/pidfd.rs | `make_pidfd_data` | 93 | 7 | yes |
| fs/pidfd.rs | `free_pidfd_data` | 99 | 13 |  |
| fs/pidfd.rs | `attach` | 111 | 10 | yes |
| fs/pidfd.rs | `data_of` | 120 | 19 |  |
| fs/pidfd.rs | `fdinfo_pid_of` | 138 | 15 | yes |
| fs/pidfd.rs | `resolve_alive` | 152 | 15 | yes |
| fs/pidfd.rs | `target_exited` | 166 | 15 |  |
| fs/pidfd.rs | `pidfd_poll` | 180 | 10 |  |
| fs/pidfd.rs | `pidfd_close` | 189 | 18 |  |
| fs/pidfd.rs | `pidfd_poll_wait` | 206 | 10 |  |
| fs/pidfd.rs | `pidfd_poll_unwait` | 215 | 10 |  |
| fs/pidfd.rs | `pidfd_dir_close` | 224 | 6 | yes |
| fs/pidfd.rs | `pidfd_dir_poll` | 229 | 4 | yes |
| fs/readahead.rs | `new` | 53 | 13 | yes |
| fs/readahead.rs | `on_read` | 65 | 41 | yes |
| fs/stat.rs | `new` | 91 | 25 | yes |
| fs/stat.rs | `set_regular_file` | 115 | 6 | yes |
| fs/stat.rs | `set_directory` | 120 | 6 |  |
| fs/stat.rs | `set_char_device` | 125 | 6 | yes |
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
| fs/stat.rs | `set_mode` | 185 | 9 | yes |
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
| fs/unsafe_edges.rs | `kernel_slice_mut` | 313 | 11 | yes |
| fs/unsafe_edges.rs | `file_dentry` | 323 | 12 | yes |
| fs/unsafe_edges.rs | `task_group_leader` | 334 | 11 | yes |
| fs/unsafe_edges.rs | `user_page_slice` | 344 | 14 | yes |
| fs/unsafe_edges.rs | `diag_read_u64` | 357 | 10 | yes |
| fs/unsafe_edges.rs | `diag_task_ref` | 366 | 12 | yes |
| fs/unsafe_edges.rs | `privdata_arc_clone` | 377 | 18 |  |
| fs/unsafe_edges.rs | `kva_copy_to` | 394 | 8 | yes |
| fs/unsafe_edges.rs | `kva_copy_from` | 401 | 8 | yes |
| fs/unsafe_edges.rs | `kva_zero` | 408 | 9 | yes |
| fs/unsafe_edges.rs | `kernel_slice_at` | 416 | 9 | yes |
| fs/unsafe_edges.rs | `file_close` | 424 | 11 | yes |
| fs/unsafe_edges.rs | `file_privdata_arc_static` | 434 | 10 |  |
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
| fs/vfs.rs | `file_opendir` | 6825 | 65 | yes |
| fs/vfs.rs | `getdents_with_dots` | 6889 | 35 | yes |
| fs/vfs.rs | `file_getdents64` | 6923 | 195 | yes |
| fs/vfs.rs | `mem_file_content` | 7117 | 8 | yes |
| fs/vfs.rs | `mem_file_content_mut` | 7124 | 9 | yes |
| fs/vfs.rs | `mem_file_take_owned` | 7132 | 8 | yes |
| fs/vfs.rs | `mem_file_read` | 7139 | 20 | yes |
| fs/vfs.rs | `mem_file_stat` | 7158 | 29 | yes |
| fs/vfs.rs | `mem_file_lseek` | 7186 | 32 | yes |
| fs/vfs.rs | `mem_file_close` | 7217 | 16 | yes |
| fs/vfs.rs | `mem_file_poll` | 7232 | 37 | yes |
| fs/vfs.rs | `vfs_fallocate` | 7268 | 39 | yes |
| fs/vfs.rs | `vfs_utimensat` | 7306 | 10 | yes |
| fs/vfs.rs | `vfs_utimensat_ex` | 7315 | 59 | yes |
| fs/vfs.rs | `open_procfs_dir` | 7373 | 16 | yes |
| fs/vfs.rs | `open_mem_file` | 7388 | 9 | yes |
| fs/vfs.rs | `open_mem_file_regen` | 7396 | 27 | yes |
| fs/vfs.rs | `pipe_fcntl` | 7422 | 13 | yes |
| fs/vt.rs | `is_vt_devno` | 123 | 7 | yes |
| fs/vt.rs | `vt_opened` | 129 | 6 | yes |
| fs/vt.rs | `vt_closed` | 134 | 6 |  |
| fs/vt.rs | `active_vt` | 139 | 6 | yes |
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
