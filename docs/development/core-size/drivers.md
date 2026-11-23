# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# drivers — hot 4816 lines (stay) / cold 13693 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| drivers/ashmem.rs | `new` | 100 | 17 | yes |
| drivers/ashmem.rs | `page_count` | 116 | 5 | yes |
| drivers/ashmem.rs | `get_page` | 120 | 7 | yes |
| drivers/ashmem.rs | `drop` | 126 | 49 | yes |
| drivers/ashmem.rs | `new` | 174 | 20 | yes |
| drivers/ashmem.rs | `range_in_range` | 193 | 10 |  |
| drivers/ashmem.rs | `pin_ranges` | 202 | 47 |  |
| drivers/ashmem.rs | `unpin_ranges` | 248 | 37 |  |
| drivers/ashmem.rs | `any_unpinned` | 284 | 14 |  |
| drivers/ashmem.rs | `area_of` | 297 | 7 |  |
| drivers/ashmem.rs | `ashmem_file_read` | 303 | 42 |  |
| drivers/ashmem.rs | `ashmem_file_lseek` | 344 | 29 |  |
| drivers/ashmem.rs | `ashmem_file_close` | 372 | 21 |  |
| drivers/ashmem.rs | `is_ashmem_file` | 392 | 9 | yes |
| drivers/ashmem.rs | `ashmem_open` | 400 | 14 |  |
| drivers/ashmem.rs | `ashmem_file_ioctl` | 413 | 14 | yes |
| drivers/ashmem.rs | `ashmem_ioctl` | 426 | 161 |  |
| drivers/ashmem.rs | `ashmem_mmap` | 586 | 163 | yes |
| drivers/blkdev/iostat.rs | `register_device` | 82 | 25 |  |
| drivers/blkdev/iostat.rs | `note_io` | 106 | 23 |  |
| drivers/blkdev/iostat.rs | `note_start` | 128 | 9 |  |
| drivers/blkdev/iostat.rs | `note_end` | 136 | 9 |  |
| drivers/blkdev/iostat.rs | `snapshot` | 144 | 20 |  |
| drivers/blkdev/mod.rs | `new` | 111 | 26 | yes |
| drivers/blkdev/mod.rs | `set_capacity` | 136 | 6 |  |
| drivers/blkdev/mod.rs | `get_capacity` | 141 | 6 | yes |
| drivers/blkdev/mod.rs | `set_private_data` | 146 | 12 | yes |
| drivers/blkdev/mod.rs | `private` | 157 | 9 |  |
| drivers/blkdev/mod.rs | `set_request_fn` | 165 | 7 |  |
| drivers/blkdev/mod.rs | `set_async_read_fn` | 171 | 9 |  |
| drivers/blkdev/mod.rs | `set_async_write_fn` | 179 | 9 |  |
| drivers/blkdev/mod.rs | `set_async_read_sg_fn` | 187 | 9 |  |
| drivers/blkdev/mod.rs | `set_async_write_sg_fn` | 195 | 9 |  |
| drivers/blkdev/mod.rs | `has_async_read_sg` | 203 | 6 | yes |
| drivers/blkdev/mod.rs | `has_async_write_sg` | 208 | 47 | yes |
| drivers/blkdev/mod.rs | `kick_all_pending_disks` | 254 | 18 | yes |
| drivers/blkdev/mod.rs | `new` | 271 | 9 | yes |
| drivers/blkdev/mod.rs | `register_disk` | 279 | 25 |  |
| drivers/blkdev/mod.rs | `get_disk` | 303 | 18 | yes |
| drivers/blkdev/mod.rs | `submit_request` | 320 | 26 |  |
| drivers/blkdev/mod.rs | `blkdev_read_async` | 345 | 71 | yes |
| drivers/blkdev/mod.rs | `blkdev_write_async` | 415 | 63 |  |
| drivers/blkdev/mod.rs | `blkdev_read_async_sg` | 477 | 50 | yes |
| drivers/blkdev/mod.rs | `blkdev_write_async_sg` | 526 | 44 | yes |
| drivers/blkdev/mod.rs | `register_disk` | 569 | 7 |  |
| drivers/blkdev/mod.rs | `get_disk` | 575 | 5 | yes |
| drivers/blkdev/mod.rs | `submit_request` | 579 | 56 |  |
| drivers/blkdev/mod.rs | `req_rwbs` | 634 | 9 |  |
| drivers/blkdev/mod.rs | `blkdev_read` | 642 | 25 | yes |
| drivers/blkdev/mod.rs | `blkdev_write` | 666 | 24 | yes |
| drivers/drm/mod.rs | `crtc_props` | 113 | 49 |  |
| drivers/drm/mod.rs | `connector_props` | 161 | 30 |  |
| drivers/drm/mod.rs | `plane_props` | 190 | 112 |  |
| drivers/drm/mod.rs | `plane_name_prop` | 301 | 79 |  |
| drivers/drm/mod.rs | `fixed_mode_name` | 379 | 9 |  |
| drivers/drm/mod.rs | `current_mode` | 387 | 33 |  |
| drivers/drm/mod.rs | `ioc` | 419 | 40 | yes |
| drivers/drm/mod.rs | `is_prime_import_request` | 458 | 11 |  |
| drivers/drm/mod.rs | `trace_foreign_prime_ioctl` | 468 | 110 | yes |
| drivers/drm/mod.rs | `drop` | 577 | 48 | yes |
| drivers/drm/mod.rs | `virtio_format_for` | 624 | 24 |  |
| drivers/drm/mod.rs | `drmdev_read` | 647 | 5 |  |
| drivers/drm/mod.rs | `drmdev_write` | 651 | 12 |  |
| drivers/drm/mod.rs | `is_drm_file` | 662 | 10 | yes |
| drivers/drm/mod.rs | `prime_read` | 671 | 5 |  |
| drivers/drm/mod.rs | `prime_write` | 675 | 6 |  |
| drivers/drm/mod.rs | `prime_close` | 680 | 20 |  |
| drivers/drm/mod.rs | `prime_lseek` | 699 | 27 |  |
| drivers/drm/mod.rs | `is_prime_file` | 725 | 18 | yes |
| drivers/drm/mod.rs | `drm_file_stat` | 742 | 37 | yes |
| drivers/drm/mod.rs | `prime_file_gem` | 778 | 15 |  |
| drivers/drm/mod.rs | `copy_in` | 792 | 17 | yes |
| drivers/drm/mod.rs | `copy_out` | 808 | 15 | yes |
| drivers/drm/mod.rs | `u32_slice_as_bytes` | 822 | 8 |  |
| drivers/drm/mod.rs | `rdu32` | 829 | 4 |  |
| drivers/drm/mod.rs | `wru32` | 832 | 4 |  |
| drivers/drm/mod.rs | `rdu64` | 835 | 4 |  |
| drivers/drm/mod.rs | `wru64` | 838 | 10 |  |
| drivers/drm/mod.rs | `drm_ioctl` | 847 | 31 | yes |
| drivers/drm/mod.rs | `trace_seq` | 877 | 46 |  |
| drivers/drm/mod.rs | `trace_ioctl` | 922 | 64 |  |
| drivers/drm/mod.rs | `drm_ioctl_inner` | 985 | 65 |  |
| drivers/drm/mod.rs | `virtgpu_getparam` | 1049 | 23 |  |
| drivers/drm/mod.rs | `rdu32_or` | 1071 | 10 |  |
| drivers/drm/mod.rs | `drm_version` | 1080 | 61 |  |
| drivers/drm/mod.rs | `drm_get_cap` | 1140 | 17 |  |
| drivers/drm/mod.rs | `drm_set_client_cap` | 1156 | 13 |  |
| drivers/drm/mod.rs | `get_resources` | 1168 | 62 |  |
| drivers/drm/mod.rs | `get_crtc` | 1229 | 16 |  |
| drivers/drm/mod.rs | `set_crtc` | 1244 | 22 |  |
| drivers/drm/mod.rs | `get_encoder` | 1265 | 16 |  |
| drivers/drm/mod.rs | `get_connector` | 1280 | 53 |  |
| drivers/drm/mod.rs | `get_plane_resources` | 1332 | 34 |  |
| drivers/drm/mod.rs | `get_plane` | 1365 | 28 |  |
| drivers/drm/mod.rs | `props_for_obj` | 1392 | 16 |  |
| drivers/drm/mod.rs | `obj_get_properties` | 1407 | 53 |  |
| drivers/drm/mod.rs | `get_property` | 1459 | 59 |  |
| drivers/drm/mod.rs | `props_for_obj_by_prop` | 1517 | 18 |  |
| drivers/drm/mod.rs | `clone_prop` | 1534 | 14 |  |
| drivers/drm/mod.rs | `create_prop_blob` | 1547 | 31 |  |
| drivers/drm/mod.rs | `destroy_prop_blob` | 1577 | 9 |  |
| drivers/drm/mod.rs | `get_prop_blob` | 1585 | 36 |  |
| drivers/drm/mod.rs | `create_dumb` | 1620 | 43 |  |
| drivers/drm/mod.rs | `gem_close` | 1662 | 47 |  |
| drivers/drm/mod.rs | `map_dumb` | 1708 | 21 |  |
| drivers/drm/mod.rs | `prime_handle_to_fd` | 1728 | 48 |  |
| drivers/drm/mod.rs | `prime_fd_to_handle` | 1775 | 109 |  |
| drivers/drm/mod.rs | `prime_file_ioctl` | 1883 | 10 | yes |
| drivers/drm/mod.rs | `addfb` | 1892 | 21 |  |
| drivers/drm/mod.rs | `addfb2` | 1912 | 15 |  |
| drivers/drm/mod.rs | `add_fb_common` | 1926 | 58 |  |
| drivers/drm/mod.rs | `rmfb` | 1983 | 20 |  |
| drivers/drm/mod.rs | `present_fb` | 2002 | 168 |  |
| drivers/drm/mod.rs | `page_flip` | 2169 | 12 |  |
| drivers/drm/mod.rs | `mode_atomic` | 2180 | 147 |  |
| drivers/drm/mod.rs | `vblank_seq_at` | 2326 | 7 |  |
| drivers/drm/mod.rs | `sleep_until_ns` | 2332 | 48 |  |
| drivers/drm/mod.rs | `wait_vblank` | 2379 | 51 |  |
| drivers/drm/mod.rs | `gem_mmap_resolve` | 2429 | 58 | yes |
| drivers/drm/mod.rs | `init_drm` | 2486 | 35 |  |
| drivers/drm/mod.rs | `gpu_pci_slot_name` | 2520 | 18 |  |
| drivers/gpu/framebuffer.rs | `new` | 63 | 11 | yes |
| drivers/gpu/framebuffer.rs | `width` | 73 | 7 |  |
| drivers/gpu/framebuffer.rs | `height` | 79 | 7 |  |
| drivers/gpu/framebuffer.rs | `stride` | 85 | 7 |  |
| drivers/gpu/framebuffer.rs | `put_pixel` | 91 | 12 |  |
| drivers/gpu/framebuffer.rs | `get_pixel` | 102 | 10 |  |
| drivers/gpu/framebuffer.rs | `fill_rect` | 111 | 13 |  |
| drivers/gpu/framebuffer.rs | `blit_rect` | 123 | 17 |  |
| drivers/gpu/framebuffer.rs | `clear` | 139 | 6 | yes |
| drivers/gpu/framebuffer.rs | `draw_line_h` | 144 | 6 |  |
| drivers/gpu/framebuffer.rs | `draw_line_v` | 149 | 6 |  |
| drivers/gpu/framebuffer.rs | `draw_line` | 154 | 34 |  |
| drivers/gpu/framebuffer.rs | `draw_circle` | 187 | 42 |  |
| drivers/gpu/framebuffer.rs | `draw_bitmap` | 228 | 20 |  |
| drivers/gpu/framebuffer.rs | `as_ptr` | 247 | 7 | yes |
| drivers/gpu/framebuffer.rs | `info` | 253 | 5 |  |
| drivers/gpu/mod.rs | `set_framebuffer_info` | 30 | 13 |  |
| drivers/gpu/mod.rs | `get_framebuffer_info` | 42 | 13 | yes |
| drivers/gpu/mod.rs | `set_gpu_device` | 54 | 7 |  |
| drivers/gpu/mod.rs | `flush_framebuffer` | 60 | 31 | yes |
| drivers/gpu/mod.rs | `with_gpu_device` | 90 | 11 |  |
| drivers/gpu/virtio_gpu.rs | `new` | 160 | 20 | yes |
| drivers/gpu/virtio_gpu.rs | `init_virtio` | 179 | 143 |  |
| drivers/gpu/virtio_gpu.rs | `init_framebuffer` | 321 | 77 |  |
| drivers/gpu/virtio_gpu.rs | `get_display_info` | 397 | 32 |  |
| drivers/gpu/virtio_gpu.rs | `create_resource_2d` | 428 | 8 |  |
| drivers/gpu/virtio_gpu.rs | `create_resource_2d_id` | 435 | 55 |  |
| drivers/gpu/virtio_gpu.rs | `attach_backing` | 489 | 6 |  |
| drivers/gpu/virtio_gpu.rs | `attach_backing_id` | 494 | 40 |  |
| drivers/gpu/virtio_gpu.rs | `set_scanout` | 533 | 36 |  |
| drivers/gpu/virtio_gpu.rs | `set_scanout_id` | 568 | 10 |  |
| drivers/gpu/virtio_gpu.rs | `set_scanout_rect` | 577 | 16 |  |
| drivers/gpu/virtio_gpu.rs | `unref_resource` | 592 | 37 |  |
| drivers/gpu/virtio_gpu.rs | `transfer_to_host_2d` | 628 | 37 |  |
| drivers/gpu/virtio_gpu.rs | `resource_flush` | 664 | 38 |  |
| drivers/gpu/virtio_gpu.rs | `transfer_to_host_2d_id` | 701 | 8 |  |
| drivers/gpu/virtio_gpu.rs | `resource_flush_id` | 708 | 7 |  |
| drivers/gpu/virtio_gpu.rs | `send_command` | 714 | 189 |  |
| drivers/gpu/virtio_gpu.rs | `pci_ecam_addr` | 902 | 5 |  |
| drivers/gpu/virtio_gpu.rs | `flush` | 906 | 11 | yes |
| drivers/gpu/virtio_gpu.rs | `get_framebuffer` | 916 | 17 |  |
| drivers/gpu/virtio_gpu.rs | `drop` | 932 | 13 | yes |
| drivers/gpu/virtio_gpu.rs | `probe_virtio_gpu` | 944 | 11 |  |
| drivers/hooks.rs | `no_char_rows` | 79 | 4 |  |
| drivers/hooks.rs | `no_loopback_init` | 82 | 4 |  |
| drivers/hooks.rs | `no_loopback_device` | 85 | 4 |  |
| drivers/hooks.rs | `no_loopback_send` | 88 | 4 |  |
| drivers/hooks.rs | `no_loopback_poll` | 91 | 4 |  |
| drivers/hooks.rs | `no_loopback_stats` | 94 | 4 |  |
| drivers/hooks.rs | `no_init_fbdev` | 97 | 4 |  |
| drivers/hooks.rs | `no_fb_file` | 100 | 4 |  |
| drivers/hooks.rs | `no_fbdev_ioctl` | 103 | 4 |  |
| drivers/hooks.rs | `no_mark_fb_user_mapped` | 106 | 2 |  |
| drivers/hooks.rs | `no_watchdog_ioctl` | 107 | 4 |  |
| drivers/hooks.rs | `no_tokenid_ioctl` | 110 | 31 |  |
| drivers/hooks.rs | `register_driver_hooks` | 140 | 6 | yes |
| drivers/hooks.rs | `hooks` | 145 | 10 |  |
| drivers/hooks.rs | `devfs_char_rows` | 154 | 6 |  |
| drivers/hooks.rs | `loopback_init` | 159 | 6 | yes |
| drivers/hooks.rs | `get_loopback_device` | 164 | 6 | yes |
| drivers/hooks.rs | `loopback_send` | 169 | 6 | yes |
| drivers/hooks.rs | `loopback_poll` | 174 | 6 | yes |
| drivers/hooks.rs | `loopback_stats` | 179 | 6 | yes |
| drivers/hooks.rs | `init_fbdev` | 184 | 6 | yes |
| drivers/hooks.rs | `is_fb_file` | 189 | 6 | yes |
| drivers/hooks.rs | `fbdev_ioctl` | 194 | 6 | yes |
| drivers/hooks.rs | `mark_fb_user_mapped` | 199 | 6 | yes |
| drivers/hooks.rs | `watchdog_ioctl` | 204 | 6 | yes |
| drivers/hooks.rs | `tokenid_ioctl` | 209 | 4 | yes |
| drivers/initcalls.rs | `register_initcall` | 66 | 21 |  |
| drivers/initcalls.rs | `run_initcalls` | 86 | 21 |  |
| drivers/initcalls.rs | `level_summary` | 106 | 13 |  |
| drivers/input/evdev.rs | `ioc_nr` | 57 | 6 |  |
| drivers/input/evdev.rs | `ioc_size` | 62 | 6 |  |
| drivers/input/evdev.rs | `ioc_dir` | 67 | 49 |  |
| drivers/input/evdev.rs | `evtype_bitmap` | 115 | 68 |  |
| drivers/input/evdev.rs | `new` | 182 | 28 | yes |
| drivers/input/evdev.rs | `set_ev_bits` | 209 | 10 |  |
| drivers/input/evdev.rs | `set_absinfo` | 218 | 21 |  |
| drivers/input/evdev.rs | `register_open_file` | 238 | 17 |  |
| drivers/input/evdev.rs | `unregister_open_file` | 254 | 21 |  |
| drivers/input/evdev.rs | `push_event` | 274 | 90 |  |
| drivers/input/evdev.rs | `wake_pollq_probed` | 363 | 94 |  |
| drivers/input/evdev.rs | `pop_event` | 456 | 31 |  |
| drivers/input/evdev.rs | `has_event` | 486 | 31 |  |
| drivers/input/evdev.rs | `evdev_keyboard` | 516 | 4 |  |
| drivers/input/evdev.rs | `evdev_pointer` | 519 | 4 |  |
| drivers/input/evdev.rs | `evdev_touchscreen` | 522 | 8 |  |
| drivers/input/evdev.rs | `file_id_is_input` | 529 | 19 | yes |
| drivers/input/evdev.rs | `latch_opener_pid` | 547 | 17 |  |
| drivers/input/evdev.rs | `evdev_opener_pid` | 563 | 9 | yes |
| drivers/input/evdev.rs | `device_of_devno` | 571 | 18 |  |
| drivers/input/evdev.rs | `device_of_file` | 588 | 27 |  |
| drivers/input/evdev.rs | `sleep_ms_interruptible` | 614 | 55 |  |
| drivers/input/evdev.rs | `node_of_file` | 668 | 24 |  |
| drivers/input/evdev.rs | `evdev_file_read` | 691 | 106 |  |
| drivers/input/evdev.rs | `trace_open` | 796 | 44 |  |
| drivers/input/evdev.rs | `evdev_file_close` | 839 | 34 |  |
| drivers/input/evdev.rs | `evdev_file_poll` | 872 | 56 |  |
| drivers/input/evdev.rs | `evdev_poll_wait` | 927 | 45 |  |
| drivers/input/evdev.rs | `evdev_poll_unwait` | 971 | 58 |  |
| drivers/input/evdev.rs | `evdev_file_write` | 1028 | 57 |  |
| drivers/input/evdev.rs | `evdev_file_ioctl` | 1084 | 47 | yes |
| drivers/input/evdev.rs | `copy_out` | 1130 | 194 | yes |
| drivers/input/evdev.rs | `format_input_bitmap` | 1323 | 2 |  |
| drivers/input/evdev.rs | `word_at` | 1324 | 35 |  |
| drivers/input/evdev.rs | `sysfs_registration` | 1358 | 51 |  |
| drivers/input/evdev.rs | `init_evdev` | 1408 | 101 |  |
| drivers/input/evdev.rs | `fill_capabilities` | 1508 | 51 |  |
| drivers/input/evdev.rs | `push_input_event` | 1558 | 150 |  |
| drivers/input/evdev.rs | `input_echo_budget` | 1707 | 20 |  |
| drivers/input/evdev.rs | `new` | 1726 | 4 | yes |
| drivers/input/evdev.rs | `push` | 1729 | 9 | yes |
| drivers/input/evdev.rs | `u64` | 1737 | 19 | yes |
| drivers/input/evdev.rs | `flush` | 1755 | 32 | yes |
| drivers/input/evdev.rs | `itt_msg` | 1786 | 15 |  |
| drivers/input/evdev.rs | `itt_u64` | 1800 | 19 |  |
| drivers/input/evdev.rs | `itt_i64` | 1818 | 14 |  |
| drivers/input/evdev.rs | `current_pid_tgid` | 1831 | 13 |  |
| drivers/input/evdev.rs | `it_msg` | 1843 | 9 |  |
| drivers/input/evdev.rs | `it_u64` | 1851 | 21 |  |
| drivers/input/evdev.rs | `it_i64` | 1871 | 26 |  |
| drivers/input/evdev.rs | `cad_detect` | 1896 | 28 |  |
| drivers/input/evdev.rs | `poll_virtio_events` | 1923 | 22 |  |
| drivers/input/event.rs | `new` | 201 | 16 | yes |
| drivers/input/event.rs | `key_event` | 216 | 6 |  |
| drivers/input/event.rs | `rel_event` | 221 | 6 |  |
| drivers/input/event.rs | `abs_event` | 226 | 6 |  |
| drivers/input/event.rs | `sync_event` | 231 | 22 |  |
| drivers/input/mod.rs | `input_debug_tick` | 81 | 18 |  |
| drivers/input/mod.rs | `input_bh_run` | 98 | 66 |  |
| drivers/input/mod.rs | `virtio_input_irq_handler` | 163 | 29 |  |
| drivers/input/mod.rs | `input_intx_service` | 191 | 13 |  |
| drivers/input/mod.rs | `register_input_irq` | 203 | 27 |  |
| drivers/input/mod.rs | `init` | 229 | 11 | yes |
| drivers/input/mod.rs | `init_virtio_input` | 239 | 58 |  |
| drivers/input/mod.rs | `poll_events` | 296 | 22 |  |
| drivers/input/mod.rs | `get_keyboard_event` | 317 | 10 |  |
| drivers/input/mod.rs | `get_pointer_event` | 326 | 8 |  |
| drivers/input/ps2.rs | `new` | 110 | 10 | yes |
| drivers/input/ps2.rs | `read_scancode` | 119 | 7 |  |
| drivers/input/ps2.rs | `has_data` | 125 | 6 |  |
| drivers/input/ps2.rs | `scancode_to_ascii` | 130 | 86 |  |
| drivers/input/ps2.rs | `new` | 215 | 14 | yes |
| drivers/input/ps2.rs | `read_event` | 228 | 7 |  |
| drivers/input/ps2.rs | `has_data` | 234 | 6 |  |
| drivers/input/ps2.rs | `x` | 239 | 6 |  |
| drivers/input/ps2.rs | `y` | 244 | 14 |  |
| drivers/input/ps2.rs | `init_keyboard` | 257 | 6 |  |
| drivers/input/ps2.rs | `init_mouse` | 262 | 4 |  |
| drivers/input/virtio_input.rs | `new` | 118 | 27 | yes |
| drivers/input/virtio_input.rs | `init_virtio` | 144 | 116 |  |
| drivers/input/virtio_input.rs | `read_device_info` | 259 | 34 |  |
| drivers/input/virtio_input.rs | `check_pointer_device` | 292 | 33 |  |
| drivers/input/virtio_input.rs | `submit_event_buffers` | 324 | 34 |  |
| drivers/input/virtio_input.rs | `read_event` | 357 | 58 |  |
| drivers/input/virtio_input.rs | `name_bytes_vec` | 414 | 9 |  |
| drivers/input/virtio_input.rs | `has_event` | 422 | 11 |  |
| drivers/input/virtio_input.rs | `name` | 432 | 6 | yes |
| drivers/input/virtio_input.rs | `is_pointer` | 437 | 7 |  |
| drivers/input/virtio_input.rs | `isr_cfg_bar` | 443 | 8 |  |
| drivers/input/virtio_input.rs | `intx_irq_line` | 450 | 10 |  |
| drivers/input/virtio_input.rs | `enable_pci_intx_irq` | 459 | 14 |  |
| drivers/input/virtio_input.rs | `read_ev_bitmap` | 472 | 31 |  |
| drivers/input/virtio_input.rs | `read_absinfo` | 502 | 38 |  |
| drivers/input/virtio_input.rs | `drop` | 539 | 17 | yes |
| drivers/input/virtio_input.rs | `probe_virtio_input_devices` | 555 | 37 |  |
| drivers/input/virtio_input.rs | `probe_virtio_input` | 591 | 12 |  |
| drivers/intc/apic.rs | `ONESHOT_LVT_LIVE_INVALIDATE` | 120 | 16 |  |
| drivers/intc/apic.rs | `reg_read` | 135 | 20 |  |
| drivers/intc/apic.rs | `reg_write` | 154 | 18 |  |
| drivers/intc/apic.rs | `eoi` | 171 | 9 | yes |
| drivers/intc/apic.rs | `ready` | 179 | 7 | yes |
| drivers/intc/apic.rs | `cpu_count` | 185 | 7 | yes |
| drivers/intc/apic.rs | `lapic_id` | 191 | 31 | yes |
| drivers/intc/apic.rs | `send_ipi` | 221 | 53 |  |
| drivers/intc/apic.rs | `kvm_para_init` | 273 | 28 |  |
| drivers/intc/apic.rs | `kvm_pv_send_ipi` | 300 | 11 |  |
| drivers/intc/apic.rs | `hc` | 310 | 37 |  |
| drivers/intc/apic.rs | `send_init` | 346 | 7 | yes |
| drivers/intc/apic.rs | `send_sipi` | 352 | 6 | yes |
| drivers/intc/apic.rs | `send_icr` | 357 | 36 |  |
| drivers/intc/apic.rs | `timer_start` | 392 | 42 | yes |
| drivers/intc/apic.rs | `timer_mask` | 433 | 29 | yes |
| drivers/intc/apic.rs | `timer_arm_oneshot` | 461 | 83 |  |
| drivers/intc/apic.rs | `tick_arm_new` | 543 | 20 |  |
| drivers/intc/apic.rs | `tick_arm_ra` | 562 | 23 |  |
| drivers/intc/apic.rs | `timer_event_record` | 584 | 31 | yes |
| drivers/intc/apic.rs | `tick_arm_ring_phys` | 614 | 43 |  |
| drivers/intc/apic.rs | `last_arm_deadline` | 656 | 32 | yes |
| drivers/intc/apic.rs | `irr_timer` | 687 | 6 |  |
| drivers/intc/apic.rs | `isr_timer` | 692 | 6 |  |
| drivers/intc/apic.rs | `lvt_masked` | 697 | 6 |  |
| drivers/intc/apic.rs | `lvt_periodic` | 702 | 8 |  |
| drivers/intc/apic.rs | `lapic_timer_snapshot` | 709 | 19 |  |
| drivers/intc/apic.rs | `tick_trap_adjudicate` | 727 | 42 |  |
| drivers/intc/apic.rs | `delay_ms` | 768 | 32 | yes |
| drivers/intc/apic.rs | `calibration_window` | 799 | 34 |  |
| drivers/intc/apic.rs | `calibrate` | 832 | 98 |  |
| drivers/intc/apic.rs | `checksum_ok` | 929 | 16 |  |
| drivers/intc/apic.rs | `read_phys_u16` | 944 | 9 |  |
| drivers/intc/apic.rs | `read_phys_u32` | 952 | 9 |  |
| drivers/intc/apic.rs | `read_phys_u64` | 960 | 10 |  |
| drivers/intc/apic.rs | `read_phys_packed_u64` | 969 | 12 |  |
| drivers/intc/apic.rs | `map_acpi_windows` | 980 | 16 |  |
| drivers/intc/apic.rs | `ensure_phys_mapped` | 995 | 40 |  |
| drivers/intc/apic.rs | `read_table_u32` | 1034 | 8 |  |
| drivers/intc/apic.rs | `read_phys_packed_u32` | 1041 | 10 |  |
| drivers/intc/apic.rs | `find_rsdp` | 1050 | 46 |  |
| drivers/intc/apic.rs | `find_table` | 1095 | 84 |  |
| drivers/intc/apic.rs | `find_hpet_base` | 1178 | 27 |  |
| drivers/intc/apic.rs | `parse_madt` | 1204 | 89 |  |
| drivers/intc/apic.rs | `this_lapic_id_raw` | 1292 | 11 |  |
| drivers/intc/apic.rs | `this_lapic_id` | 1302 | 26 |  |
| drivers/intc/apic.rs | `init_x2apic_or_mmio` | 1327 | 43 |  |
| drivers/intc/apic.rs | `init` | 1369 | 50 | yes |
| drivers/intc/apic.rs | `init_secondary` | 1418 | 18 | yes |
| drivers/intc/clint.rs | `init` | 32 | 16 | yes |
| drivers/intc/clint.rs | `send_ipi` | 47 | 20 |  |
| drivers/intc/clint.rs | `clear_ipi` | 66 | 21 |  |
| drivers/intc/clint.rs | `get_ipi_count` | 86 | 15 |  |
| drivers/intc/clint.rs | `read_time` | 100 | 21 | yes |
| drivers/intc/clint.rs | `set_timecmp` | 120 | 10 |  |
| drivers/intc/gicv3.rs | `default_priority_for` | 147 | 18 |  |
| drivers/intc/gicv3.rs | `reg_read32` | 164 | 6 |  |
| drivers/intc/gicv3.rs | `reg_write32` | 169 | 6 |  |
| drivers/intc/gicv3.rs | `reg_read64` | 174 | 6 |  |
| drivers/intc/gicv3.rs | `reg_write64` | 179 | 7 |  |
| drivers/intc/gicv3.rs | `wait_not_set` | 185 | 16 |  |
| drivers/intc/gicv3.rs | `iar1_read` | 200 | 12 |  |
| drivers/intc/gicv3.rs | `eoir1_write` | 211 | 20 |  |
| drivers/intc/gicv3.rs | `send_sgi` | 230 | 21 |  |
| drivers/intc/gicv3.rs | `dist_init` | 250 | 66 |  |
| drivers/intc/gicv3.rs | `find_my_rd_base` | 315 | 38 |  |
| drivers/intc/gicv3.rs | `redist_configure_local` | 352 | 43 |  |
| drivers/intc/gicv3.rs | `enable_cpu_interface` | 394 | 27 | yes |
| drivers/intc/gicv3.rs | `enable_ppi` | 420 | 13 | yes |
| drivers/intc/gicv3.rs | `gic_mask` | 432 | 19 |  |
| drivers/intc/gicv3.rs | `gic_unmask` | 450 | 16 |  |
| drivers/intc/gicv3.rs | `gic_eoi` | 465 | 15 |  |
| drivers/intc/gicv3.rs | `gic_irq_map` | 479 | 5 |  |
| drivers/intc/gicv3.rs | `gic_irq_unmap` | 483 | 13 |  |
| drivers/intc/gicv3.rs | `claim` | 495 | 19 |  |
| drivers/intc/gicv3.rs | `eoi` | 513 | 7 |  |
| drivers/intc/gicv3.rs | `init` | 519 | 40 | yes |
| drivers/intc/mod.rs | `init` | 28 | 6 | yes |
| drivers/intc/mod.rs | `init` | 33 | 10 | yes |
| drivers/intc/mod.rs | `init` | 42 | 4 | yes |
| drivers/intc/plic.rs | `s_mode_ctx` | 49 | 10 |  |
| drivers/intc/plic.rs | `default_priority_for` | 58 | 17 |  |
| drivers/intc/plic.rs | `new` | 74 | 6 | yes |
| drivers/intc/plic.rs | `init` | 79 | 24 | yes |
| drivers/intc/plic.rs | `set_priority` | 102 | 7 |  |
| drivers/intc/plic.rs | `set_threshold` | 108 | 14 |  |
| drivers/intc/plic.rs | `enable_interrupt` | 121 | 16 |  |
| drivers/intc/plic.rs | `disable_interrupt` | 136 | 13 |  |
| drivers/intc/plic.rs | `claim` | 148 | 9 | yes |
| drivers/intc/plic.rs | `complete` | 156 | 14 | yes |
| drivers/intc/plic.rs | `read_pending_word` | 169 | 13 |  |
| drivers/intc/plic.rs | `read_pending` | 181 | 5 |  |
| drivers/intc/plic.rs | `trigger_ipi` | 185 | 20 |  |
| drivers/intc/plic.rs | `plic_mask` | 204 | 7 |  |
| drivers/intc/plic.rs | `plic_unmask` | 210 | 15 |  |
| drivers/intc/plic.rs | `plic_eoi` | 224 | 30 |  |
| drivers/intc/plic.rs | `after_smp_online_restore` | 253 | 26 | yes |
| drivers/intc/plic.rs | `plic_irq_map` | 278 | 6 |  |
| drivers/intc/plic.rs | `plic_irq_unmap` | 283 | 13 |  |
| drivers/intc/plic.rs | `init` | 295 | 26 | yes |
| drivers/intc/plic.rs | `claim` | 320 | 5 | yes |
| drivers/intc/plic.rs | `complete` | 324 | 5 | yes |
| drivers/intc/plic.rs | `enable_interrupt` | 328 | 5 |  |
| drivers/intc/plic.rs | `read_pending` | 332 | 5 |  |
| drivers/intc/plic.rs | `trigger_ipi` | 336 | 12 |  |
| drivers/intc/plic.rs | `vw_plic_dump` | 347 | 35 |  |
| drivers/intc/plic.rs | `self_addr_enable` | 381 | 5 |  |
| drivers/intc/plic.rs | `read_threshold` | 385 | 9 |  |
| drivers/loop_dev.rs | `new` | 105 | 27 | yes |
| drivers/loop_dev.rs | `is_loop_devno` | 131 | 8 |  |
| drivers/loop_dev.rs | `init_loops` | 138 | 48 |  |
| drivers/loop_dev.rs | `loop_disk` | 185 | 9 | yes |
| drivers/loop_dev.rs | `loop_static_name` | 193 | 6 |  |
| drivers/loop_dev.rs | `loop_get_free` | 198 | 14 |  |
| drivers/loop_dev.rs | `loop_set_fd` | 211 | 34 |  |
| drivers/loop_dev.rs | `loop_set_fd_named` | 244 | 13 |  |
| drivers/loop_dev.rs | `loop_clr_fd` | 256 | 38 |  |
| drivers/loop_dev.rs | `loop_size_bytes` | 293 | 17 |  |
| drivers/loop_dev.rs | `loop_is_bound` | 309 | 20 |  |
| drivers/loop_dev.rs | `loop_io_claim_held_by_current` | 328 | 16 | yes |
| drivers/loop_dev.rs | `file_size_bytes` | 343 | 8 |  |
| drivers/loop_dev.rs | `return_errno` | 350 | 8 |  |
| drivers/loop_dev.rs | `loop_request_fn` | 357 | 60 |  |
| drivers/loop_dev.rs | `loop_request_fn_claimed` | 416 | 56 |  |
| drivers/loop_dev.rs | `loop_devno_of` | 471 | 17 |  |
| drivers/loop_dev.rs | `loop_file_read` | 487 | 45 |  |
| drivers/loop_dev.rs | `loop_file_write` | 531 | 56 |  |
| drivers/loop_dev.rs | `partitions_lines` | 586 | 59 |  |
| drivers/loop_dev.rs | `loop_status_of` | 644 | 33 |  |
| drivers/loop_dev.rs | `loop_change_fd` | 676 | 20 |  |
| drivers/loop_dev.rs | `loop_set_capacity` | 695 | 16 |  |
| drivers/loop_dev.rs | `loop_file_ioctl` | 710 | 312 | yes |
| drivers/loop_dev.rs | `fd_backing_path` | 1021 | 7 |  |
| drivers/loop_dev.rs | `loop_offset_of` | 1027 | 10 |  |
| drivers/loop_dev.rs | `loop_sizelimit_of` | 1036 | 9 |  |
| drivers/loop_dev.rs | `set_loop_geometry` | 1044 | 14 |  |
| drivers/loop_dev.rs | `loop_file_lseek` | 1057 | 24 |  |
| drivers/net/space.rs | `set_address` | 138 | 7 |  |
| drivers/net/space.rs | `get_name` | 144 | 16 | yes |
| drivers/net/space.rs | `xmit` | 159 | 6 | yes |
| drivers/net/space.rs | `get_stats` | 164 | 10 | yes |
| drivers/net/space.rs | `up` | 173 | 6 | yes |
| drivers/net/space.rs | `down` | 178 | 6 | yes |
| drivers/net/space.rs | `is_up` | 183 | 6 |  |
| drivers/net/space.rs | `is_running` | 188 | 17 | yes |
| drivers/net/space.rs | `register_netdevice` | 204 | 16 |  |
| drivers/net/space.rs | `unregister_netdevice` | 219 | 14 |  |
| drivers/net/space.rs | `get_netdevice_by_index` | 232 | 18 |  |
| drivers/net/space.rs | `get_netdevice_by_name` | 249 | 12 |  |
| drivers/net/space.rs | `get_netdevice_count` | 260 | 4 |  |
| drivers/net/virtio_net.rs | `new` | 78 | 17 | yes |
| drivers/net/virtio_net.rs | `init` | 94 | 242 | yes |
| drivers/net/virtio_net.rs | `get_mac` | 335 | 6 | yes |
| drivers/net/virtio_net.rs | `get_mtu` | 340 | 12 |  |
| drivers/net/virtio_net.rs | `xmit` | 351 | 148 | yes |
| drivers/net/virtio_net.rs | `poll` | 498 | 123 | yes |
| drivers/net/virtio_net.rs | `recycle_rx_buffer` | 620 | 11 |  |
| drivers/net/virtio_net.rs | `dealloc_rx_buffer` | 630 | 15 |  |
| drivers/net/virtio_net.rs | `refill_rx_buffers` | 644 | 58 |  |
| drivers/net/virtio_net.rs | `get_stats` | 701 | 7 | yes |
| drivers/net/virtio_net.rs | `virtio_net_xmit` | 707 | 12 |  |
| drivers/net/virtio_net.rs | `virtio_net_get_stats` | 718 | 36 |  |
| drivers/net/virtio_net.rs | `init` | 753 | 14 | yes |
| drivers/net/virtio_net.rs | `publish_eth0` | 766 | 37 |  |
| drivers/net/virtio_net.rs | `get_device` | 802 | 6 | yes |
| drivers/net/virtio_net.rs | `get_net_device` | 807 | 6 |  |
| drivers/net/virtio_net.rs | `get_device_base_addr` | 812 | 11 |  |
| drivers/net/virtio_net.rs | `interrupt_handler` | 822 | 30 |  |
| drivers/net/virtio_net.rs | `net_rx_softirq_handler` | 851 | 8 |  |
| drivers/net/virtio_net.rs | `enable_device_interrupt` | 858 | 39 |  |
| drivers/net/virtio_net.rs | `pci_select_queue` | 896 | 17 |  |
| drivers/net/virtio_net.rs | `pci_set_queue_size` | 912 | 15 |  |
| drivers/net/virtio_net.rs | `init_pci` | 926 | 154 |  |
| drivers/net/virtio_net.rs | `intx_net_service` | 1079 | 12 |  |
| drivers/net/virtio_net.rs | `interrupt_handler_pci` | 1090 | 19 |  |
| drivers/pci/mod.rs | `empty` | 85 | 12 | yes |
| drivers/pci/mod.rs | `is_valid` | 96 | 18 | yes |
| drivers/pci/mod.rs | `ecam_va` | 113 | 5 |  |
| drivers/pci/mod.rs | `ecam_va` | 117 | 8 |  |
| drivers/pci/mod.rs | `ecam_va` | 124 | 7 |  |
| drivers/pci/mod.rs | `new` | 130 | 7 | yes |
| drivers/pci/mod.rs | `new_at_ecam` | 136 | 8 |  |
| drivers/pci/mod.rs | `cfg` | 143 | 6 | yes |
| drivers/pci/mod.rs | `read_config_dword` | 148 | 6 |  |
| drivers/pci/mod.rs | `write_config_dword` | 153 | 6 |  |
| drivers/pci/mod.rs | `read_config_word` | 158 | 6 |  |
| drivers/pci/mod.rs | `write_config_word` | 163 | 6 |  |
| drivers/pci/mod.rs | `read_config_byte` | 168 | 6 |  |
| drivers/pci/mod.rs | `read_bar` | 173 | 51 |  |
| drivers/pci/mod.rs | `probe_bar_size` | 223 | 66 |  |
| drivers/pci/mod.rs | `assign_bar` | 288 | 72 |  |
| drivers/pci/mod.rs | `vendor_id` | 359 | 6 |  |
| drivers/pci/mod.rs | `device_id` | 364 | 6 |  |
| drivers/pci/mod.rs | `class_code` | 369 | 6 |  |
| drivers/pci/mod.rs | `subclass` | 374 | 6 |  |
| drivers/pci/mod.rs | `prog_if` | 379 | 6 |  |
| drivers/pci/mod.rs | `revision_id` | 384 | 6 |  |
| drivers/pci/mod.rs | `interrupt_pin` | 389 | 6 |  |
| drivers/pci/mod.rs | `interrupt_line` | 394 | 6 |  |
| drivers/pci/mod.rs | `set_command` | 399 | 6 |  |
| drivers/pci/mod.rs | `command` | 404 | 6 |  |
| drivers/pci/mod.rs | `enable_bus_master` | 409 | 62 |  |
| drivers/pci/mod.rs | `ecam_base` | 470 | 5 |  |
| drivers/pci/mod.rs | `find_ecam_devices` | 474 | 40 |  |
| drivers/pci/mod.rs | `enumerate_virtio_devices` | 513 | 56 |  |
| drivers/rtc.rs | `read_reg` | 61 | 10 |  |
| drivers/rtc.rs | `rtc_init_alias` | 70 | 16 |  |
| drivers/rtc.rs | `read_ns` | 85 | 28 |  |
| drivers/rtc.rs | `rtc_init_wall_clock` | 112 | 28 |  |
| drivers/rtc.rs | `wall_secs` | 139 | 18 | yes |
| drivers/rtc.rs | `tick_refresh_cached_secs` | 156 | 8 |  |
| drivers/rtc.rs | `cached_wall_secs` | 163 | 4 | yes |
| drivers/timer/aarch64.rs | `ticks_per_period` | 29 | 7 |  |
| drivers/timer/aarch64.rs | `clock_freq` | 35 | 22 |  |
| drivers/timer/aarch64.rs | `get_jiffies` | 56 | 12 | yes |
| drivers/timer/aarch64.rs | `test_advance_jiffies` | 67 | 12 |  |
| drivers/timer/aarch64.rs | `jiffy_floor_of_time` | 78 | 9 |  |
| drivers/timer/aarch64.rs | `coarse_ns_since_boot` | 86 | 8 | yes |
| drivers/timer/aarch64.rs | `increment_jiffies` | 93 | 7 |  |
| drivers/timer/aarch64.rs | `jiffies_to_msecs` | 99 | 7 | yes |
| drivers/timer/aarch64.rs | `msecs_to_jiffies` | 105 | 7 | yes |
| drivers/timer/aarch64.rs | `read_time` | 111 | 8 | yes |
| drivers/timer/aarch64.rs | `monotonic_ns_since_boot` | 118 | 8 | yes |
| drivers/timer/aarch64.rs | `set_timer` | 125 | 17 |  |
| drivers/timer/aarch64.rs | `set_next_trigger` | 141 | 13 | yes |
| drivers/timer/aarch64.rs | `rearm_for_hres` | 153 | 6 | yes |
| drivers/timer/aarch64.rs | `timer_interrupt_handler` | 158 | 29 | yes |
| drivers/timer/hpet.rs | `wild_guard_range` | 90 | 15 |  |
| drivers/timer/hpet.rs | `BASE_LOCKED` | 104 | 6 |  |
| drivers/timer/hpet.rs | `BASE_LOCKED_INV` | 109 | 7 |  |
| drivers/timer/hpet.rs | `DIV` | 115 | 8 |  |
| drivers/timer/hpet.rs | `DIV_INV` | 122 | 8 |  |
| drivers/timer/hpet.rs | `BOOT_OFFSET` | 129 | 6 |  |
| drivers/timer/hpet.rs | `BOOT_OFFSET_INV` | 134 | 23 |  |
| drivers/timer/hpet.rs | `heal_report` | 156 | 37 |  |
| drivers/timer/hpet.rs | `validated_read_base` | 192 | 39 |  |
| drivers/timer/hpet.rs | `guard_armed` | 230 | 14 |  |
| drivers/timer/hpet.rs | `ready` | 243 | 9 | yes |
| drivers/timer/hpet.rs | `r32` | 251 | 11 |  |
| drivers/timer/hpet.rs | `w32` | 261 | 14 |  |
| drivers/timer/hpet.rs | `read_counter` | 274 | 34 | yes |
| drivers/timer/hpet.rs | `read_time_nominal` | 307 | 72 | yes |
| drivers/timer/hpet.rs | `init` | 378 | 132 | yes |
| drivers/timer/hpet.rs | `calibrate_tsc` | 509 | 56 |  |
| drivers/timer/hpet.rs | `read_counter_raw` | 564 | 15 |  |
| drivers/timer/riscv64.rs | `get_jiffies` | 47 | 12 | yes |
| drivers/timer/riscv64.rs | `test_advance_jiffies` | 58 | 12 |  |
| drivers/timer/riscv64.rs | `jiffy_floor_of_time` | 69 | 18 |  |
| drivers/timer/riscv64.rs | `coarse_ns_since_boot` | 86 | 14 | yes |
| drivers/timer/riscv64.rs | `increment_jiffies` | 99 | 8 |  |
| drivers/timer/riscv64.rs | `jiffies_to_msecs` | 106 | 8 | yes |
| drivers/timer/riscv64.rs | `msecs_to_jiffies` | 113 | 7 | yes |
| drivers/timer/riscv64.rs | `read_time` | 119 | 12 | yes |
| drivers/timer/riscv64.rs | `monotonic_ns_since_boot` | 130 | 9 | yes |
| drivers/timer/riscv64.rs | `set_timer_stimecmp` | 138 | 13 |  |
| drivers/timer/riscv64.rs | `set_timer_sbi` | 150 | 10 |  |
| drivers/timer/riscv64.rs | `set_timer` | 159 | 14 |  |
| drivers/timer/riscv64.rs | `set_next_trigger` | 172 | 14 | yes |
| drivers/timer/riscv64.rs | `rearm_for_hres` | 185 | 20 | yes |
| drivers/timer/riscv64.rs | `timer_interrupt_handler` | 204 | 51 | yes |
| drivers/timer/x86_64.rs | `jiffy_floor_of_time` | 133 | 8 |  |
| drivers/timer/x86_64.rs | `coarse_ns_since_boot` | 140 | 11 | yes |
| drivers/timer/x86_64.rs | `monotonic_ns_since_boot` | 150 | 36 | yes |
| drivers/timer/x86_64.rs | `read_time` | 185 | 70 | yes |
| drivers/timer/x86_64.rs | `promote_tsc_timekeeper` | 254 | 20 |  |
| drivers/timer/x86_64.rs | `resync_tsc_against_hpet` | 273 | 61 | yes |
| drivers/timer/x86_64.rs | `calibrate_cpu_tsc_offset` | 333 | 41 | yes |
| drivers/timer/x86_64.rs | `resync_report` | 373 | 14 |  |
| drivers/timer/x86_64.rs | `tsc_active` | 386 | 11 |  |
| drivers/timer/x86_64.rs | `lapic_time_calibrated` | 396 | 23 |  |
| drivers/timer/x86_64.rs | `init` | 418 | 17 | yes |
| drivers/timer/x86_64.rs | `set_timer` | 434 | 21 |  |
| drivers/timer/x86_64.rs | `set_next_trigger` | 454 | 29 | yes |
| drivers/timer/x86_64.rs | `rearm_for_hres` | 482 | 15 | yes |
| drivers/timer/x86_64.rs | `timebase_precise` | 496 | 7 |  |
| drivers/timer/x86_64.rs | `pre_hpet_time` | 502 | 13 |  |
| drivers/timer/x86_64.rs | `increment_jiffies_from_timebase` | 514 | 9 |  |
| drivers/timer/x86_64.rs | `timer_interrupt_handler` | 522 | 69 | yes |
| drivers/virtio/mod.rs | `new` | 94 | 14 | yes |
| drivers/virtio/mod.rs | `init` | 107 | 209 | yes |
| drivers/virtio/mod.rs | `get_capacity` | 315 | 8 | yes |
| drivers/virtio/mod.rs | `handle_request` | 322 | 40 |  |
| drivers/virtio/mod.rs | `read_block` | 361 | 182 |  |
| drivers/virtio/mod.rs | `write_block` | 542 | 179 |  |
| drivers/virtio/mod.rs | `virtio_blk_kick_op` | 720 | 30 |  |
| drivers/virtio/mod.rs | `submit_read_async` | 749 | 137 |  |
| drivers/virtio/mod.rs | `submit_write_async` | 885 | 121 |  |
| drivers/virtio/mod.rs | `async_read_fn` | 1005 | 18 |  |
| drivers/virtio/mod.rs | `async_write_fn` | 1022 | 75 |  |
| drivers/virtio/mod.rs | `io_buf_alloc` | 1096 | 22 |  |
| drivers/virtio/mod.rs | `io_buf_free` | 1117 | 23 |  |
| drivers/virtio/mod.rs | `io_buf_init` | 1139 | 26 |  |
| drivers/virtio/mod.rs | `io_buf_release` | 1164 | 20 |  |
| drivers/virtio/mod.rs | `begin_delivery` | 1183 | 7 |  |
| drivers/virtio/mod.rs | `deliver_completion` | 1189 | 84 |  |
| drivers/virtio/mod.rs | `pci_blk_acquire_site` | 1272 | 24 |  |
| drivers/virtio/mod.rs | `wedge_put` | 1295 | 9 |  |
| drivers/virtio/mod.rs | `wedge_hex` | 1303 | 20 |  |
| drivers/virtio/mod.rs | `wedge_dec` | 1322 | 22 |  |
| drivers/virtio/mod.rs | `pci_blk_contended_forensics` | 1343 | 80 |  |
| drivers/virtio/mod.rs | `arm` | 1422 | 27 |  |
| drivers/virtio/mod.rs | `drop` | 1448 | 14 | yes |
| drivers/virtio/mod.rs | `pci_blk_lock` | 1461 | 40 |  |
| drivers/virtio/mod.rs | `pci_blk_lock_defer` | 1500 | 21 |  |
| drivers/virtio/mod.rs | `pci_blk_holder_watchdog` | 1520 | 44 |  |
| drivers/virtio/probe.rs | `virtio_mmio_va` | 56 | 15 |  |
| drivers/virtio/probe.rs | `virtio_probe_devices` | 70 | 71 |  |
| drivers/virtio/probe.rs | `init_virtio_net` | 140 | 18 |  |
| drivers/virtio/probe.rs | `init_virtio_blk` | 157 | 32 |  |
| drivers/virtio/probe.rs | `init_loopback_device` | 188 | 20 |  |
| drivers/virtio/probe.rs | `init_network_devices` | 207 | 33 |  |
| drivers/virtio/probe.rs | `init_pci_net_devices` | 239 | 57 |  |
| drivers/virtio/probe.rs | `register_probe_initcalls` | 295 | 23 |  |
| drivers/virtio/probe.rs | `init_block_devices_actual` | 317 | 5 |  |
| drivers/virtio/probe.rs | `init_block_devices` | 321 | 26 |  |
| drivers/virtio/probe.rs | `init_pci_block_devices` | 346 | 282 |  |
| drivers/virtio/probe.rs | `pci_dev_hierarchy` | 627 | 47 |  |
| drivers/virtio/probe.rs | `pci_rescan_block_hotplug` | 673 | 104 |  |
| drivers/virtio/queue.rs | `used_ring_idx` | 156 | 15 |  |
| drivers/virtio/queue.rs | `used_ring_elem_id` | 170 | 17 |  |
| drivers/virtio/queue.rs | `resp_done` | 186 | 7 |  |
| drivers/virtio/queue.rs | `resp_status_byte` | 192 | 35 |  |
| drivers/virtio/queue.rs | `new` | 226 | 6 | yes |
| drivers/virtio/queue.rs | `with_notify_width` | 231 | 114 |  |
| drivers/virtio/queue.rs | `free_vring` | 344 | 12 |  |
| drivers/virtio/queue.rs | `get_avail` | 355 | 11 |  |
| drivers/virtio/queue.rs | `get_used` | 365 | 16 |  |
| drivers/virtio/queue.rs | `get_used_elem` | 380 | 9 |  |
| drivers/virtio/queue.rs | `get_last_used` | 388 | 12 |  |
| drivers/virtio/queue.rs | `notify` | 399 | 72 | yes |
| drivers/virtio/queue.rs | `wait_for_completion` | 470 | 14 |  |
| drivers/virtio/queue.rs | `wait_for_completion_max` | 483 | 30 |  |
| drivers/virtio/queue.rs | `used_ring_ptr` | 512 | 10 |  |
| drivers/virtio/queue.rs | `avail_shadow_snapshot` | 521 | 21 |  |
| drivers/virtio/queue.rs | `vw_pollstall_print` | 541 | 9 |  |
| drivers/virtio/queue.rs | `pdec` | 549 | 96 | yes |
| drivers/virtio/queue.rs | `wait_for_used_interruptible` | 644 | 402 |  |
| drivers/virtio/queue.rs | `wait_for_desc_completion` | 1045 | 101 |  |
| drivers/virtio/queue.rs | `submit_quiet` | 1145 | 32 |  |
| drivers/virtio/queue.rs | `submit` | 1176 | 12 | yes |
| drivers/virtio/queue.rs | `get_desc` | 1187 | 10 |  |
| drivers/virtio/queue.rs | `alloc_desc` | 1196 | 14 |  |
| drivers/virtio/queue.rs | `alloc_desc_chain` | 1209 | 18 |  |
| drivers/virtio/queue.rs | `alloc_desc_run_checked` | 1226 | 62 |  |
| drivers/virtio/queue.rs | `alloc_desc_run` | 1287 | 11 |  |
| drivers/virtio/queue.rs | `hold_descs` | 1297 | 8 |  |
| drivers/virtio/queue.rs | `release_descs` | 1304 | 14 |  |
| drivers/virtio/queue.rs | `descs_held_snapshot` | 1317 | 11 |  |
| drivers/virtio/queue.rs | `reclaim_descs` | 1327 | 20 |  |
| drivers/virtio/queue.rs | `note_timed_out_chain` | 1346 | 26 |  |
| drivers/virtio/virtio_pci.rs | `device_alias_oob_warned` | 19 | 98 |  |
| drivers/virtio/virtio_pci.rs | `common_cfg` | 116 | 12 |  |
| drivers/virtio/virtio_pci.rs | `find_virtio_capability` | 127 | 49 |  |
| drivers/virtio/virtio_pci.rs | `read_virtio_cap` | 175 | 32 |  |
| drivers/virtio/virtio_pci.rs | `new` | 206 | 255 | yes |
| drivers/virtio/virtio_pci.rs | `intx_ack_only_service` | 460 | 6 |  |
