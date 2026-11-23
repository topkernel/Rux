# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# drivers — hot 3635 lines (stay) / cold 14229 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| drivers/ashmem.rs | `new` | 100 | 16 | yes |
| drivers/ashmem.rs | `page_count` | 116 | 4 |  |
| drivers/ashmem.rs | `get_page` | 120 | 6 | yes |
| drivers/ashmem.rs | `drop` | 126 | 48 | yes |
| drivers/ashmem.rs | `new` | 174 | 19 | yes |
| drivers/ashmem.rs | `range_in_range` | 193 | 9 |  |
| drivers/ashmem.rs | `pin_ranges` | 202 | 46 |  |
| drivers/ashmem.rs | `unpin_ranges` | 248 | 36 |  |
| drivers/ashmem.rs | `any_unpinned` | 284 | 13 |  |
| drivers/ashmem.rs | `area_of` | 297 | 6 |  |
| drivers/ashmem.rs | `ashmem_file_read` | 303 | 41 |  |
| drivers/ashmem.rs | `ashmem_file_lseek` | 344 | 28 |  |
| drivers/ashmem.rs | `ashmem_file_close` | 372 | 20 |  |
| drivers/ashmem.rs | `is_ashmem_file` | 392 | 8 |  |
| drivers/ashmem.rs | `ashmem_open` | 400 | 13 |  |
| drivers/ashmem.rs | `ashmem_file_ioctl` | 413 | 13 |  |
| drivers/ashmem.rs | `ashmem_ioctl` | 426 | 160 |  |
| drivers/ashmem.rs | `ashmem_mmap` | 586 | 162 |  |
| drivers/blkdev/iostat.rs | `register_device` | 82 | 24 |  |
| drivers/blkdev/iostat.rs | `note_io` | 106 | 22 |  |
| drivers/blkdev/iostat.rs | `note_start` | 128 | 8 |  |
| drivers/blkdev/iostat.rs | `note_end` | 136 | 8 |  |
| drivers/blkdev/iostat.rs | `snapshot` | 144 | 19 |  |
| drivers/blkdev/mod.rs | `new` | 111 | 25 | yes |
| drivers/blkdev/mod.rs | `set_capacity` | 136 | 5 |  |
| drivers/blkdev/mod.rs | `get_capacity` | 141 | 5 |  |
| drivers/blkdev/mod.rs | `set_private_data` | 146 | 11 | yes |
| drivers/blkdev/mod.rs | `private` | 157 | 8 | yes |
| drivers/blkdev/mod.rs | `set_request_fn` | 165 | 6 |  |
| drivers/blkdev/mod.rs | `set_async_read_fn` | 171 | 8 |  |
| drivers/blkdev/mod.rs | `set_async_write_fn` | 179 | 8 |  |
| drivers/blkdev/mod.rs | `set_async_read_sg_fn` | 187 | 8 |  |
| drivers/blkdev/mod.rs | `set_async_write_sg_fn` | 195 | 8 |  |
| drivers/blkdev/mod.rs | `has_async_read_sg` | 203 | 5 |  |
| drivers/blkdev/mod.rs | `has_async_write_sg` | 208 | 46 | yes |
| drivers/blkdev/mod.rs | `kick_all_pending_disks` | 254 | 17 | yes |
| drivers/blkdev/mod.rs | `new` | 271 | 8 | yes |
| drivers/blkdev/mod.rs | `register_disk` | 279 | 24 |  |
| drivers/blkdev/mod.rs | `get_disk` | 303 | 17 |  |
| drivers/blkdev/mod.rs | `submit_request` | 320 | 25 |  |
| drivers/blkdev/mod.rs | `blkdev_read_async` | 345 | 70 | yes |
| drivers/blkdev/mod.rs | `blkdev_write_async` | 415 | 62 | yes |
| drivers/blkdev/mod.rs | `blkdev_read_async_sg` | 477 | 49 |  |
| drivers/blkdev/mod.rs | `blkdev_write_async_sg` | 526 | 43 | yes |
| drivers/blkdev/mod.rs | `register_disk` | 569 | 6 |  |
| drivers/blkdev/mod.rs | `get_disk` | 575 | 4 |  |
| drivers/blkdev/mod.rs | `submit_request` | 579 | 55 |  |
| drivers/blkdev/mod.rs | `req_rwbs` | 634 | 8 |  |
| drivers/blkdev/mod.rs | `blkdev_read` | 642 | 24 | yes |
| drivers/blkdev/mod.rs | `blkdev_write` | 666 | 23 | yes |
| drivers/drm/mod.rs | `crtc_props` | 113 | 48 |  |
| drivers/drm/mod.rs | `connector_props` | 161 | 29 |  |
| drivers/drm/mod.rs | `plane_props` | 190 | 111 |  |
| drivers/drm/mod.rs | `plane_name_prop` | 301 | 78 |  |
| drivers/drm/mod.rs | `fixed_mode_name` | 379 | 8 |  |
| drivers/drm/mod.rs | `current_mode` | 387 | 32 |  |
| drivers/drm/mod.rs | `ioc` | 419 | 39 |  |
| drivers/drm/mod.rs | `is_prime_import_request` | 458 | 10 |  |
| drivers/drm/mod.rs | `trace_foreign_prime_ioctl` | 468 | 109 |  |
| drivers/drm/mod.rs | `drop` | 577 | 47 | yes |
| drivers/drm/mod.rs | `virtio_format_for` | 624 | 23 |  |
| drivers/drm/mod.rs | `drmdev_read` | 647 | 4 |  |
| drivers/drm/mod.rs | `drmdev_write` | 651 | 11 |  |
| drivers/drm/mod.rs | `is_drm_file` | 662 | 9 |  |
| drivers/drm/mod.rs | `prime_read` | 671 | 4 |  |
| drivers/drm/mod.rs | `prime_write` | 675 | 5 |  |
| drivers/drm/mod.rs | `prime_close` | 680 | 19 |  |
| drivers/drm/mod.rs | `prime_lseek` | 699 | 26 |  |
| drivers/drm/mod.rs | `is_prime_file` | 725 | 17 |  |
| drivers/drm/mod.rs | `drm_file_stat` | 742 | 36 | yes |
| drivers/drm/mod.rs | `prime_file_gem` | 778 | 14 |  |
| drivers/drm/mod.rs | `copy_in` | 792 | 16 |  |
| drivers/drm/mod.rs | `copy_out` | 808 | 14 | yes |
| drivers/drm/mod.rs | `u32_slice_as_bytes` | 822 | 7 |  |
| drivers/drm/mod.rs | `rdu32` | 829 | 3 |  |
| drivers/drm/mod.rs | `wru32` | 832 | 3 |  |
| drivers/drm/mod.rs | `rdu64` | 835 | 3 |  |
| drivers/drm/mod.rs | `wru64` | 838 | 9 |  |
| drivers/drm/mod.rs | `drm_ioctl` | 847 | 30 |  |
| drivers/drm/mod.rs | `trace_seq` | 877 | 45 |  |
| drivers/drm/mod.rs | `trace_ioctl` | 922 | 63 |  |
| drivers/drm/mod.rs | `drm_ioctl_inner` | 985 | 64 |  |
| drivers/drm/mod.rs | `virtgpu_getparam` | 1049 | 22 |  |
| drivers/drm/mod.rs | `rdu32_or` | 1071 | 9 |  |
| drivers/drm/mod.rs | `drm_version` | 1080 | 60 |  |
| drivers/drm/mod.rs | `drm_get_cap` | 1140 | 16 |  |
| drivers/drm/mod.rs | `drm_set_client_cap` | 1156 | 12 |  |
| drivers/drm/mod.rs | `get_resources` | 1168 | 61 |  |
| drivers/drm/mod.rs | `get_crtc` | 1229 | 15 |  |
| drivers/drm/mod.rs | `set_crtc` | 1244 | 21 |  |
| drivers/drm/mod.rs | `get_encoder` | 1265 | 15 |  |
| drivers/drm/mod.rs | `get_connector` | 1280 | 52 |  |
| drivers/drm/mod.rs | `get_plane_resources` | 1332 | 33 |  |
| drivers/drm/mod.rs | `get_plane` | 1365 | 27 |  |
| drivers/drm/mod.rs | `props_for_obj` | 1392 | 15 |  |
| drivers/drm/mod.rs | `obj_get_properties` | 1407 | 52 |  |
| drivers/drm/mod.rs | `get_property` | 1459 | 58 |  |
| drivers/drm/mod.rs | `props_for_obj_by_prop` | 1517 | 17 |  |
| drivers/drm/mod.rs | `clone_prop` | 1534 | 13 |  |
| drivers/drm/mod.rs | `create_prop_blob` | 1547 | 30 |  |
| drivers/drm/mod.rs | `destroy_prop_blob` | 1577 | 8 |  |
| drivers/drm/mod.rs | `get_prop_blob` | 1585 | 35 |  |
| drivers/drm/mod.rs | `create_dumb` | 1620 | 42 |  |
| drivers/drm/mod.rs | `gem_close` | 1662 | 46 |  |
| drivers/drm/mod.rs | `map_dumb` | 1708 | 20 |  |
| drivers/drm/mod.rs | `prime_handle_to_fd` | 1728 | 47 |  |
| drivers/drm/mod.rs | `prime_fd_to_handle` | 1775 | 108 |  |
| drivers/drm/mod.rs | `prime_file_ioctl` | 1883 | 9 |  |
| drivers/drm/mod.rs | `addfb` | 1892 | 20 |  |
| drivers/drm/mod.rs | `addfb2` | 1912 | 14 |  |
| drivers/drm/mod.rs | `add_fb_common` | 1926 | 57 |  |
| drivers/drm/mod.rs | `rmfb` | 1983 | 19 |  |
| drivers/drm/mod.rs | `present_fb` | 2002 | 167 |  |
| drivers/drm/mod.rs | `page_flip` | 2169 | 11 |  |
| drivers/drm/mod.rs | `mode_atomic` | 2180 | 146 |  |
| drivers/drm/mod.rs | `vblank_seq_at` | 2326 | 6 |  |
| drivers/drm/mod.rs | `sleep_until_ns` | 2332 | 47 |  |
| drivers/drm/mod.rs | `wait_vblank` | 2379 | 50 |  |
| drivers/drm/mod.rs | `gem_mmap_resolve` | 2429 | 57 |  |
| drivers/drm/mod.rs | `init_drm` | 2486 | 34 |  |
| drivers/drm/mod.rs | `gpu_pci_slot_name` | 2520 | 17 |  |
| drivers/gpu/framebuffer.rs | `new` | 63 | 10 | yes |
| drivers/gpu/framebuffer.rs | `width` | 73 | 6 |  |
| drivers/gpu/framebuffer.rs | `height` | 79 | 6 |  |
| drivers/gpu/framebuffer.rs | `stride` | 85 | 6 |  |
| drivers/gpu/framebuffer.rs | `put_pixel` | 91 | 11 |  |
| drivers/gpu/framebuffer.rs | `get_pixel` | 102 | 9 |  |
| drivers/gpu/framebuffer.rs | `fill_rect` | 111 | 12 |  |
| drivers/gpu/framebuffer.rs | `blit_rect` | 123 | 16 |  |
| drivers/gpu/framebuffer.rs | `clear` | 139 | 5 | yes |
| drivers/gpu/framebuffer.rs | `draw_line_h` | 144 | 5 |  |
| drivers/gpu/framebuffer.rs | `draw_line_v` | 149 | 5 |  |
| drivers/gpu/framebuffer.rs | `draw_line` | 154 | 33 |  |
| drivers/gpu/framebuffer.rs | `draw_circle` | 187 | 41 |  |
| drivers/gpu/framebuffer.rs | `draw_bitmap` | 228 | 19 |  |
| drivers/gpu/framebuffer.rs | `as_ptr` | 247 | 6 | yes |
| drivers/gpu/framebuffer.rs | `info` | 253 | 4 |  |
| drivers/gpu/mod.rs | `set_framebuffer_info` | 30 | 12 |  |
| drivers/gpu/mod.rs | `get_framebuffer_info` | 42 | 12 |  |
| drivers/gpu/mod.rs | `set_gpu_device` | 54 | 6 |  |
| drivers/gpu/mod.rs | `flush_framebuffer` | 60 | 30 |  |
| drivers/gpu/mod.rs | `with_gpu_device` | 90 | 10 |  |
| drivers/gpu/virtio_gpu.rs | `new` | 160 | 19 | yes |
| drivers/gpu/virtio_gpu.rs | `init_virtio` | 179 | 142 |  |
| drivers/gpu/virtio_gpu.rs | `init_framebuffer` | 321 | 76 |  |
| drivers/gpu/virtio_gpu.rs | `get_display_info` | 397 | 31 |  |
| drivers/gpu/virtio_gpu.rs | `create_resource_2d` | 428 | 7 |  |
| drivers/gpu/virtio_gpu.rs | `create_resource_2d_id` | 435 | 54 |  |
| drivers/gpu/virtio_gpu.rs | `attach_backing` | 489 | 5 |  |
| drivers/gpu/virtio_gpu.rs | `attach_backing_id` | 494 | 39 |  |
| drivers/gpu/virtio_gpu.rs | `set_scanout` | 533 | 35 |  |
| drivers/gpu/virtio_gpu.rs | `set_scanout_id` | 568 | 9 |  |
| drivers/gpu/virtio_gpu.rs | `set_scanout_rect` | 577 | 15 |  |
| drivers/gpu/virtio_gpu.rs | `unref_resource` | 592 | 36 |  |
| drivers/gpu/virtio_gpu.rs | `transfer_to_host_2d` | 628 | 36 |  |
| drivers/gpu/virtio_gpu.rs | `resource_flush` | 664 | 37 |  |
| drivers/gpu/virtio_gpu.rs | `transfer_to_host_2d_id` | 701 | 7 |  |
| drivers/gpu/virtio_gpu.rs | `resource_flush_id` | 708 | 6 |  |
| drivers/gpu/virtio_gpu.rs | `send_command` | 714 | 188 |  |
| drivers/gpu/virtio_gpu.rs | `pci_ecam_addr` | 902 | 4 |  |
| drivers/gpu/virtio_gpu.rs | `flush` | 906 | 10 | yes |
| drivers/gpu/virtio_gpu.rs | `get_framebuffer` | 916 | 16 |  |
| drivers/gpu/virtio_gpu.rs | `drop` | 932 | 12 | yes |
| drivers/gpu/virtio_gpu.rs | `probe_virtio_gpu` | 944 | 10 |  |
| drivers/hooks.rs | `no_char_rows` | 79 | 3 |  |
| drivers/hooks.rs | `no_loopback_init` | 82 | 3 |  |
| drivers/hooks.rs | `no_loopback_device` | 85 | 3 |  |
| drivers/hooks.rs | `no_loopback_send` | 88 | 3 |  |
| drivers/hooks.rs | `no_loopback_poll` | 91 | 3 |  |
| drivers/hooks.rs | `no_loopback_stats` | 94 | 3 |  |
| drivers/hooks.rs | `no_init_fbdev` | 97 | 3 |  |
| drivers/hooks.rs | `no_fb_file` | 100 | 3 |  |
| drivers/hooks.rs | `no_fbdev_ioctl` | 103 | 3 |  |
| drivers/hooks.rs | `no_mark_fb_user_mapped` | 106 | 1 |  |
| drivers/hooks.rs | `no_watchdog_ioctl` | 107 | 3 |  |
| drivers/hooks.rs | `no_tokenid_ioctl` | 110 | 30 |  |
| drivers/hooks.rs | `register_driver_hooks` | 140 | 5 |  |
| drivers/hooks.rs | `hooks` | 145 | 9 |  |
| drivers/hooks.rs | `devfs_char_rows` | 154 | 5 |  |
| drivers/hooks.rs | `loopback_init` | 159 | 5 |  |
| drivers/hooks.rs | `get_loopback_device` | 164 | 5 |  |
| drivers/hooks.rs | `loopback_send` | 169 | 5 |  |
| drivers/hooks.rs | `loopback_poll` | 174 | 5 |  |
| drivers/hooks.rs | `loopback_stats` | 179 | 5 |  |
| drivers/hooks.rs | `init_fbdev` | 184 | 5 |  |
| drivers/hooks.rs | `is_fb_file` | 189 | 5 |  |
| drivers/hooks.rs | `fbdev_ioctl` | 194 | 5 |  |
| drivers/hooks.rs | `mark_fb_user_mapped` | 199 | 5 |  |
| drivers/hooks.rs | `watchdog_ioctl` | 204 | 5 |  |
| drivers/hooks.rs | `tokenid_ioctl` | 209 | 3 |  |
| drivers/initcalls.rs | `register_initcall` | 66 | 20 |  |
| drivers/initcalls.rs | `run_initcalls` | 86 | 20 |  |
| drivers/initcalls.rs | `level_summary` | 106 | 12 |  |
| drivers/input/evdev.rs | `ioc_nr` | 57 | 5 |  |
| drivers/input/evdev.rs | `ioc_size` | 62 | 5 |  |
| drivers/input/evdev.rs | `ioc_dir` | 67 | 48 |  |
| drivers/input/evdev.rs | `evtype_bitmap` | 115 | 67 |  |
| drivers/input/evdev.rs | `new` | 182 | 27 | yes |
| drivers/input/evdev.rs | `set_ev_bits` | 209 | 9 |  |
| drivers/input/evdev.rs | `set_absinfo` | 218 | 20 |  |
| drivers/input/evdev.rs | `register_open_file` | 238 | 16 |  |
| drivers/input/evdev.rs | `unregister_open_file` | 254 | 20 |  |
| drivers/input/evdev.rs | `push_event` | 274 | 89 |  |
| drivers/input/evdev.rs | `wake_pollq_probed` | 363 | 93 |  |
| drivers/input/evdev.rs | `pop_event` | 456 | 30 |  |
| drivers/input/evdev.rs | `has_event` | 486 | 30 |  |
| drivers/input/evdev.rs | `evdev_keyboard` | 516 | 3 |  |
| drivers/input/evdev.rs | `evdev_pointer` | 519 | 3 |  |
| drivers/input/evdev.rs | `evdev_touchscreen` | 522 | 7 |  |
| drivers/input/evdev.rs | `file_id_is_input` | 529 | 18 |  |
| drivers/input/evdev.rs | `latch_opener_pid` | 547 | 16 |  |
| drivers/input/evdev.rs | `evdev_opener_pid` | 563 | 8 |  |
| drivers/input/evdev.rs | `device_of_devno` | 571 | 17 |  |
| drivers/input/evdev.rs | `device_of_file` | 588 | 26 |  |
| drivers/input/evdev.rs | `sleep_ms_interruptible` | 614 | 54 |  |
| drivers/input/evdev.rs | `node_of_file` | 668 | 23 |  |
| drivers/input/evdev.rs | `evdev_file_read` | 691 | 105 |  |
| drivers/input/evdev.rs | `trace_open` | 796 | 43 |  |
| drivers/input/evdev.rs | `evdev_file_close` | 839 | 33 |  |
| drivers/input/evdev.rs | `evdev_file_poll` | 872 | 55 |  |
| drivers/input/evdev.rs | `evdev_poll_wait` | 927 | 44 |  |
| drivers/input/evdev.rs | `evdev_poll_unwait` | 971 | 57 |  |
| drivers/input/evdev.rs | `evdev_file_write` | 1028 | 56 |  |
| drivers/input/evdev.rs | `evdev_file_ioctl` | 1084 | 46 |  |
| drivers/input/evdev.rs | `copy_out` | 1130 | 193 | yes |
| drivers/input/evdev.rs | `format_input_bitmap` | 1323 | 1 |  |
| drivers/input/evdev.rs | `word_at` | 1324 | 34 |  |
| drivers/input/evdev.rs | `sysfs_registration` | 1358 | 50 |  |
| drivers/input/evdev.rs | `init_evdev` | 1408 | 100 |  |
| drivers/input/evdev.rs | `fill_capabilities` | 1508 | 50 |  |
| drivers/input/evdev.rs | `push_input_event` | 1558 | 149 |  |
| drivers/input/evdev.rs | `input_echo_budget` | 1707 | 19 |  |
| drivers/input/evdev.rs | `new` | 1726 | 3 | yes |
| drivers/input/evdev.rs | `push` | 1729 | 8 | yes |
| drivers/input/evdev.rs | `u64` | 1737 | 18 |  |
| drivers/input/evdev.rs | `flush` | 1755 | 31 | yes |
| drivers/input/evdev.rs | `itt_msg` | 1786 | 14 |  |
| drivers/input/evdev.rs | `itt_u64` | 1800 | 18 |  |
| drivers/input/evdev.rs | `itt_i64` | 1818 | 13 |  |
| drivers/input/evdev.rs | `current_pid_tgid` | 1831 | 12 |  |
| drivers/input/evdev.rs | `it_msg` | 1843 | 8 |  |
| drivers/input/evdev.rs | `it_u64` | 1851 | 20 |  |
| drivers/input/evdev.rs | `it_i64` | 1871 | 25 |  |
| drivers/input/evdev.rs | `cad_detect` | 1896 | 27 |  |
| drivers/input/evdev.rs | `poll_virtio_events` | 1923 | 21 |  |
| drivers/input/event.rs | `new` | 201 | 15 | yes |
| drivers/input/event.rs | `key_event` | 216 | 5 |  |
| drivers/input/event.rs | `rel_event` | 221 | 5 |  |
| drivers/input/event.rs | `abs_event` | 226 | 5 |  |
| drivers/input/event.rs | `sync_event` | 231 | 21 |  |
| drivers/input/mod.rs | `input_debug_tick` | 81 | 17 | yes |
| drivers/input/mod.rs | `input_bh_run` | 98 | 65 |  |
| drivers/input/mod.rs | `virtio_input_irq_handler` | 163 | 28 |  |
| drivers/input/mod.rs | `input_intx_service` | 191 | 12 |  |
| drivers/input/mod.rs | `register_input_irq` | 203 | 26 |  |
| drivers/input/mod.rs | `init` | 229 | 10 | yes |
| drivers/input/mod.rs | `init_virtio_input` | 239 | 57 |  |
| drivers/input/mod.rs | `poll_events` | 296 | 21 |  |
| drivers/input/mod.rs | `get_keyboard_event` | 317 | 9 |  |
| drivers/input/mod.rs | `get_pointer_event` | 326 | 7 |  |
| drivers/input/ps2.rs | `new` | 110 | 9 | yes |
| drivers/input/ps2.rs | `read_scancode` | 119 | 6 |  |
| drivers/input/ps2.rs | `has_data` | 125 | 5 |  |
| drivers/input/ps2.rs | `scancode_to_ascii` | 130 | 85 |  |
| drivers/input/ps2.rs | `new` | 215 | 13 | yes |
| drivers/input/ps2.rs | `read_event` | 228 | 6 |  |
| drivers/input/ps2.rs | `has_data` | 234 | 5 |  |
| drivers/input/ps2.rs | `x` | 239 | 5 |  |
| drivers/input/ps2.rs | `y` | 244 | 13 |  |
| drivers/input/ps2.rs | `init_keyboard` | 257 | 5 |  |
| drivers/input/ps2.rs | `init_mouse` | 262 | 3 |  |
| drivers/input/virtio_input.rs | `new` | 118 | 26 | yes |
| drivers/input/virtio_input.rs | `init_virtio` | 144 | 115 |  |
| drivers/input/virtio_input.rs | `read_device_info` | 259 | 33 |  |
| drivers/input/virtio_input.rs | `check_pointer_device` | 292 | 32 |  |
| drivers/input/virtio_input.rs | `submit_event_buffers` | 324 | 33 |  |
| drivers/input/virtio_input.rs | `read_event` | 357 | 57 |  |
| drivers/input/virtio_input.rs | `name_bytes_vec` | 414 | 8 |  |
| drivers/input/virtio_input.rs | `has_event` | 422 | 10 |  |
| drivers/input/virtio_input.rs | `name` | 432 | 5 | yes |
| drivers/input/virtio_input.rs | `is_pointer` | 437 | 6 |  |
| drivers/input/virtio_input.rs | `isr_cfg_bar` | 443 | 7 |  |
| drivers/input/virtio_input.rs | `intx_irq_line` | 450 | 9 |  |
| drivers/input/virtio_input.rs | `enable_pci_intx_irq` | 459 | 13 |  |
| drivers/input/virtio_input.rs | `read_ev_bitmap` | 472 | 30 |  |
| drivers/input/virtio_input.rs | `read_absinfo` | 502 | 37 |  |
| drivers/input/virtio_input.rs | `drop` | 539 | 16 | yes |
| drivers/input/virtio_input.rs | `probe_virtio_input_devices` | 555 | 36 |  |
| drivers/input/virtio_input.rs | `probe_virtio_input` | 591 | 11 |  |
| drivers/intc/apic.rs | `ONESHOT_LVT_LIVE_INVALIDATE` | 120 | 15 |  |
| drivers/intc/apic.rs | `reg_read` | 135 | 19 |  |
| drivers/intc/apic.rs | `reg_write` | 154 | 17 |  |
| drivers/intc/apic.rs | `eoi` | 171 | 8 | yes |
| drivers/intc/apic.rs | `ready` | 179 | 6 | yes |
| drivers/intc/apic.rs | `cpu_count` | 185 | 6 | yes |
| drivers/intc/apic.rs | `lapic_id` | 191 | 30 | yes |
| drivers/intc/apic.rs | `send_ipi` | 221 | 52 | yes |
| drivers/intc/apic.rs | `kvm_para_init` | 273 | 27 |  |
| drivers/intc/apic.rs | `kvm_pv_send_ipi` | 300 | 10 |  |
| drivers/intc/apic.rs | `hc` | 310 | 36 |  |
| drivers/intc/apic.rs | `send_init` | 346 | 6 | yes |
| drivers/intc/apic.rs | `send_sipi` | 352 | 5 | yes |
| drivers/intc/apic.rs | `send_icr` | 357 | 35 |  |
| drivers/intc/apic.rs | `timer_start` | 392 | 41 | yes |
| drivers/intc/apic.rs | `timer_mask` | 433 | 28 | yes |
| drivers/intc/apic.rs | `timer_arm_oneshot` | 461 | 82 |  |
| drivers/intc/apic.rs | `tick_arm_new` | 543 | 19 |  |
| drivers/intc/apic.rs | `tick_arm_ra` | 562 | 22 |  |
| drivers/intc/apic.rs | `timer_event_record` | 584 | 30 | yes |
| drivers/intc/apic.rs | `tick_arm_ring_phys` | 614 | 42 |  |
| drivers/intc/apic.rs | `last_arm_deadline` | 656 | 31 | yes |
| drivers/intc/apic.rs | `irr_timer` | 687 | 5 |  |
| drivers/intc/apic.rs | `isr_timer` | 692 | 5 |  |
| drivers/intc/apic.rs | `lvt_masked` | 697 | 5 |  |
| drivers/intc/apic.rs | `lvt_periodic` | 702 | 7 |  |
| drivers/intc/apic.rs | `lapic_timer_snapshot` | 709 | 18 | yes |
| drivers/intc/apic.rs | `tick_trap_adjudicate` | 727 | 41 | yes |
| drivers/intc/apic.rs | `delay_ms` | 768 | 31 | yes |
| drivers/intc/apic.rs | `calibration_window` | 799 | 33 |  |
| drivers/intc/apic.rs | `calibrate` | 832 | 97 |  |
| drivers/intc/apic.rs | `checksum_ok` | 929 | 15 |  |
| drivers/intc/apic.rs | `read_phys_u16` | 944 | 8 |  |
| drivers/intc/apic.rs | `read_phys_u32` | 952 | 8 |  |
| drivers/intc/apic.rs | `read_phys_u64` | 960 | 9 |  |
| drivers/intc/apic.rs | `read_phys_packed_u64` | 969 | 11 |  |
| drivers/intc/apic.rs | `map_acpi_windows` | 980 | 15 |  |
| drivers/intc/apic.rs | `ensure_phys_mapped` | 995 | 39 |  |
| drivers/intc/apic.rs | `read_table_u32` | 1034 | 7 |  |
| drivers/intc/apic.rs | `read_phys_packed_u32` | 1041 | 9 |  |
| drivers/intc/apic.rs | `find_rsdp` | 1050 | 45 |  |
| drivers/intc/apic.rs | `find_table` | 1095 | 83 |  |
| drivers/intc/apic.rs | `find_hpet_base` | 1178 | 26 |  |
| drivers/intc/apic.rs | `parse_madt` | 1204 | 88 |  |
| drivers/intc/apic.rs | `this_lapic_id_raw` | 1292 | 10 |  |
| drivers/intc/apic.rs | `this_lapic_id` | 1302 | 25 |  |
| drivers/intc/apic.rs | `init_x2apic_or_mmio` | 1327 | 42 |  |
| drivers/intc/apic.rs | `init` | 1369 | 49 | yes |
| drivers/intc/apic.rs | `init_secondary` | 1418 | 17 | yes |
| drivers/intc/clint.rs | `init` | 32 | 15 | yes |
| drivers/intc/clint.rs | `send_ipi` | 47 | 19 | yes |
| drivers/intc/clint.rs | `clear_ipi` | 66 | 20 |  |
| drivers/intc/clint.rs | `get_ipi_count` | 86 | 14 |  |
| drivers/intc/clint.rs | `read_time` | 100 | 20 | yes |
| drivers/intc/clint.rs | `set_timecmp` | 120 | 9 |  |
| drivers/intc/gicv3.rs | `default_priority_for` | 147 | 17 |  |
| drivers/intc/gicv3.rs | `reg_read32` | 164 | 5 |  |
| drivers/intc/gicv3.rs | `reg_write32` | 169 | 5 |  |
| drivers/intc/gicv3.rs | `reg_read64` | 174 | 5 |  |
| drivers/intc/gicv3.rs | `reg_write64` | 179 | 6 |  |
| drivers/intc/gicv3.rs | `wait_not_set` | 185 | 15 |  |
| drivers/intc/gicv3.rs | `iar1_read` | 200 | 11 |  |
| drivers/intc/gicv3.rs | `eoir1_write` | 211 | 19 |  |
| drivers/intc/gicv3.rs | `send_sgi` | 230 | 20 |  |
| drivers/intc/gicv3.rs | `dist_init` | 250 | 65 |  |
| drivers/intc/gicv3.rs | `find_my_rd_base` | 315 | 37 |  |
| drivers/intc/gicv3.rs | `redist_configure_local` | 352 | 42 |  |
| drivers/intc/gicv3.rs | `enable_cpu_interface` | 394 | 26 | yes |
| drivers/intc/gicv3.rs | `enable_ppi` | 420 | 12 | yes |
| drivers/intc/gicv3.rs | `gic_mask` | 432 | 18 |  |
| drivers/intc/gicv3.rs | `gic_unmask` | 450 | 15 |  |
| drivers/intc/gicv3.rs | `gic_eoi` | 465 | 14 |  |
| drivers/intc/gicv3.rs | `gic_irq_map` | 479 | 4 |  |
| drivers/intc/gicv3.rs | `gic_irq_unmap` | 483 | 12 |  |
| drivers/intc/gicv3.rs | `claim` | 495 | 18 | yes |
| drivers/intc/gicv3.rs | `eoi` | 513 | 6 | yes |
| drivers/intc/gicv3.rs | `init` | 519 | 39 | yes |
| drivers/intc/mod.rs | `init` | 28 | 5 | yes |
| drivers/intc/mod.rs | `init` | 33 | 9 | yes |
| drivers/intc/mod.rs | `init` | 42 | 3 | yes |
| drivers/intc/plic.rs | `s_mode_ctx` | 49 | 9 |  |
| drivers/intc/plic.rs | `default_priority_for` | 58 | 16 |  |
| drivers/intc/plic.rs | `new` | 74 | 5 | yes |
| drivers/intc/plic.rs | `init` | 79 | 23 | yes |
| drivers/intc/plic.rs | `set_priority` | 102 | 6 |  |
| drivers/intc/plic.rs | `set_threshold` | 108 | 13 |  |
| drivers/intc/plic.rs | `enable_interrupt` | 121 | 15 |  |
| drivers/intc/plic.rs | `disable_interrupt` | 136 | 12 |  |
| drivers/intc/plic.rs | `claim` | 148 | 8 | yes |
| drivers/intc/plic.rs | `complete` | 156 | 13 | yes |
| drivers/intc/plic.rs | `read_pending_word` | 169 | 12 |  |
| drivers/intc/plic.rs | `read_pending` | 181 | 4 |  |
| drivers/intc/plic.rs | `trigger_ipi` | 185 | 19 |  |
| drivers/intc/plic.rs | `plic_mask` | 204 | 6 |  |
| drivers/intc/plic.rs | `plic_unmask` | 210 | 14 |  |
| drivers/intc/plic.rs | `plic_eoi` | 224 | 29 |  |
| drivers/intc/plic.rs | `after_smp_online_restore` | 253 | 25 | yes |
| drivers/intc/plic.rs | `plic_irq_map` | 278 | 5 |  |
| drivers/intc/plic.rs | `plic_irq_unmap` | 283 | 12 |  |
| drivers/intc/plic.rs | `init` | 295 | 25 | yes |
| drivers/intc/plic.rs | `claim` | 320 | 4 | yes |
| drivers/intc/plic.rs | `complete` | 324 | 4 | yes |
| drivers/intc/plic.rs | `enable_interrupt` | 328 | 4 |  |
| drivers/intc/plic.rs | `read_pending` | 332 | 4 |  |
| drivers/intc/plic.rs | `trigger_ipi` | 336 | 11 |  |
| drivers/intc/plic.rs | `vw_plic_dump` | 347 | 34 |  |
| drivers/intc/plic.rs | `self_addr_enable` | 381 | 4 |  |
| drivers/intc/plic.rs | `read_threshold` | 385 | 8 |  |
| drivers/loop_dev.rs | `new` | 105 | 26 | yes |
| drivers/loop_dev.rs | `is_loop_devno` | 131 | 7 |  |
| drivers/loop_dev.rs | `init_loops` | 138 | 47 |  |
| drivers/loop_dev.rs | `loop_disk` | 185 | 8 |  |
| drivers/loop_dev.rs | `loop_static_name` | 193 | 5 |  |
| drivers/loop_dev.rs | `loop_get_free` | 198 | 13 |  |
| drivers/loop_dev.rs | `loop_set_fd` | 211 | 33 |  |
| drivers/loop_dev.rs | `loop_set_fd_named` | 244 | 12 |  |
| drivers/loop_dev.rs | `loop_clr_fd` | 256 | 37 |  |
| drivers/loop_dev.rs | `loop_size_bytes` | 293 | 16 |  |
| drivers/loop_dev.rs | `loop_is_bound` | 309 | 19 |  |
| drivers/loop_dev.rs | `loop_io_claim_held_by_current` | 328 | 15 | yes |
| drivers/loop_dev.rs | `file_size_bytes` | 343 | 7 |  |
| drivers/loop_dev.rs | `return_errno` | 350 | 7 |  |
| drivers/loop_dev.rs | `loop_request_fn` | 357 | 59 |  |
| drivers/loop_dev.rs | `loop_request_fn_claimed` | 416 | 55 |  |
| drivers/loop_dev.rs | `loop_devno_of` | 471 | 16 |  |
| drivers/loop_dev.rs | `loop_file_read` | 487 | 44 |  |
| drivers/loop_dev.rs | `loop_file_write` | 531 | 55 |  |
| drivers/loop_dev.rs | `partitions_lines` | 586 | 58 |  |
| drivers/loop_dev.rs | `loop_status_of` | 644 | 32 |  |
| drivers/loop_dev.rs | `loop_change_fd` | 676 | 19 |  |
| drivers/loop_dev.rs | `loop_set_capacity` | 695 | 15 |  |
| drivers/loop_dev.rs | `loop_file_ioctl` | 710 | 311 |  |
| drivers/loop_dev.rs | `fd_backing_path` | 1021 | 6 |  |
| drivers/loop_dev.rs | `loop_offset_of` | 1027 | 9 |  |
| drivers/loop_dev.rs | `loop_sizelimit_of` | 1036 | 8 |  |
| drivers/loop_dev.rs | `set_loop_geometry` | 1044 | 13 |  |
| drivers/loop_dev.rs | `loop_file_lseek` | 1057 | 23 |  |
| drivers/net/space.rs | `set_address` | 138 | 6 |  |
| drivers/net/space.rs | `get_name` | 144 | 15 | yes |
| drivers/net/space.rs | `xmit` | 159 | 5 |  |
| drivers/net/space.rs | `get_stats` | 164 | 9 |  |
| drivers/net/space.rs | `up` | 173 | 5 |  |
| drivers/net/space.rs | `down` | 178 | 5 |  |
| drivers/net/space.rs | `is_up` | 183 | 5 |  |
| drivers/net/space.rs | `is_running` | 188 | 16 | yes |
| drivers/net/space.rs | `register_netdevice` | 204 | 15 |  |
| drivers/net/space.rs | `unregister_netdevice` | 219 | 13 |  |
| drivers/net/space.rs | `get_netdevice_by_index` | 232 | 17 |  |
| drivers/net/space.rs | `get_netdevice_by_name` | 249 | 11 |  |
| drivers/net/space.rs | `get_netdevice_count` | 260 | 3 |  |
| drivers/net/virtio_net.rs | `new` | 78 | 16 | yes |
| drivers/net/virtio_net.rs | `init` | 94 | 241 | yes |
| drivers/net/virtio_net.rs | `get_mac` | 335 | 5 |  |
| drivers/net/virtio_net.rs | `get_mtu` | 340 | 11 |  |
| drivers/net/virtio_net.rs | `xmit` | 351 | 147 |  |
| drivers/net/virtio_net.rs | `poll` | 498 | 122 | yes |
| drivers/net/virtio_net.rs | `recycle_rx_buffer` | 620 | 10 |  |
| drivers/net/virtio_net.rs | `dealloc_rx_buffer` | 630 | 14 |  |
| drivers/net/virtio_net.rs | `refill_rx_buffers` | 644 | 57 |  |
| drivers/net/virtio_net.rs | `get_stats` | 701 | 6 |  |
| drivers/net/virtio_net.rs | `virtio_net_xmit` | 707 | 11 |  |
| drivers/net/virtio_net.rs | `virtio_net_get_stats` | 718 | 35 |  |
| drivers/net/virtio_net.rs | `init` | 753 | 13 | yes |
| drivers/net/virtio_net.rs | `publish_eth0` | 766 | 36 |  |
| drivers/net/virtio_net.rs | `get_device` | 802 | 5 |  |
| drivers/net/virtio_net.rs | `get_net_device` | 807 | 5 |  |
| drivers/net/virtio_net.rs | `get_device_base_addr` | 812 | 10 |  |
| drivers/net/virtio_net.rs | `interrupt_handler` | 822 | 29 |  |
| drivers/net/virtio_net.rs | `net_rx_softirq_handler` | 851 | 7 |  |
| drivers/net/virtio_net.rs | `enable_device_interrupt` | 858 | 38 |  |
| drivers/net/virtio_net.rs | `pci_select_queue` | 896 | 16 |  |
| drivers/net/virtio_net.rs | `pci_set_queue_size` | 912 | 14 |  |
| drivers/net/virtio_net.rs | `init_pci` | 926 | 153 |  |
| drivers/net/virtio_net.rs | `intx_net_service` | 1079 | 11 |  |
| drivers/net/virtio_net.rs | `interrupt_handler_pci` | 1090 | 18 |  |
| drivers/pci/mod.rs | `empty` | 85 | 11 | yes |
| drivers/pci/mod.rs | `is_valid` | 96 | 17 | yes |
| drivers/pci/mod.rs | `ecam_va` | 113 | 4 |  |
| drivers/pci/mod.rs | `ecam_va` | 117 | 7 |  |
| drivers/pci/mod.rs | `ecam_va` | 124 | 6 |  |
| drivers/pci/mod.rs | `new` | 130 | 6 | yes |
| drivers/pci/mod.rs | `new_at_ecam` | 136 | 7 |  |
| drivers/pci/mod.rs | `cfg` | 143 | 5 | yes |
| drivers/pci/mod.rs | `read_config_dword` | 148 | 5 |  |
| drivers/pci/mod.rs | `write_config_dword` | 153 | 5 |  |
| drivers/pci/mod.rs | `read_config_word` | 158 | 5 |  |
| drivers/pci/mod.rs | `write_config_word` | 163 | 5 |  |
| drivers/pci/mod.rs | `read_config_byte` | 168 | 5 |  |
| drivers/pci/mod.rs | `read_bar` | 173 | 50 |  |
| drivers/pci/mod.rs | `probe_bar_size` | 223 | 65 |  |
| drivers/pci/mod.rs | `assign_bar` | 288 | 71 |  |
| drivers/pci/mod.rs | `vendor_id` | 359 | 5 |  |
| drivers/pci/mod.rs | `device_id` | 364 | 5 |  |
| drivers/pci/mod.rs | `class_code` | 369 | 5 |  |
| drivers/pci/mod.rs | `subclass` | 374 | 5 |  |
| drivers/pci/mod.rs | `prog_if` | 379 | 5 |  |
| drivers/pci/mod.rs | `revision_id` | 384 | 5 |  |
| drivers/pci/mod.rs | `interrupt_pin` | 389 | 5 |  |
| drivers/pci/mod.rs | `interrupt_line` | 394 | 5 |  |
| drivers/pci/mod.rs | `set_command` | 399 | 5 |  |
| drivers/pci/mod.rs | `command` | 404 | 5 |  |
| drivers/pci/mod.rs | `enable_bus_master` | 409 | 61 |  |
| drivers/pci/mod.rs | `ecam_base` | 470 | 4 |  |
| drivers/pci/mod.rs | `find_ecam_devices` | 474 | 39 |  |
| drivers/pci/mod.rs | `enumerate_virtio_devices` | 513 | 55 |  |
| drivers/rtc.rs | `read_reg` | 61 | 9 |  |
| drivers/rtc.rs | `rtc_init_alias` | 70 | 15 |  |
| drivers/rtc.rs | `read_ns` | 85 | 27 |  |
| drivers/rtc.rs | `rtc_init_wall_clock` | 112 | 27 |  |
| drivers/rtc.rs | `wall_secs` | 139 | 17 |  |
| drivers/rtc.rs | `tick_refresh_cached_secs` | 156 | 7 |  |
| drivers/rtc.rs | `cached_wall_secs` | 163 | 3 |  |
| drivers/timer/aarch64.rs | `ticks_per_period` | 29 | 6 |  |
| drivers/timer/aarch64.rs | `clock_freq` | 35 | 21 |  |
| drivers/timer/aarch64.rs | `get_jiffies` | 56 | 11 | yes |
| drivers/timer/aarch64.rs | `test_advance_jiffies` | 67 | 11 | yes |
| drivers/timer/aarch64.rs | `jiffy_floor_of_time` | 78 | 8 | yes |
| drivers/timer/aarch64.rs | `coarse_ns_since_boot` | 86 | 7 | yes |
| drivers/timer/aarch64.rs | `increment_jiffies` | 93 | 6 |  |
| drivers/timer/aarch64.rs | `jiffies_to_msecs` | 99 | 6 | yes |
| drivers/timer/aarch64.rs | `msecs_to_jiffies` | 105 | 6 | yes |
| drivers/timer/aarch64.rs | `read_time` | 111 | 7 | yes |
| drivers/timer/aarch64.rs | `monotonic_ns_since_boot` | 118 | 7 | yes |
| drivers/timer/aarch64.rs | `set_timer` | 125 | 16 |  |
| drivers/timer/aarch64.rs | `set_next_trigger` | 141 | 12 | yes |
| drivers/timer/aarch64.rs | `rearm_for_hres` | 153 | 5 |  |
| drivers/timer/aarch64.rs | `timer_interrupt_handler` | 158 | 28 | yes |
| drivers/timer/hpet.rs | `wild_guard_range` | 90 | 14 |  |
| drivers/timer/hpet.rs | `BASE_LOCKED` | 104 | 5 |  |
| drivers/timer/hpet.rs | `BASE_LOCKED_INV` | 109 | 6 |  |
| drivers/timer/hpet.rs | `DIV` | 115 | 7 |  |
| drivers/timer/hpet.rs | `DIV_INV` | 122 | 7 |  |
| drivers/timer/hpet.rs | `BOOT_OFFSET` | 129 | 5 |  |
| drivers/timer/hpet.rs | `BOOT_OFFSET_INV` | 134 | 22 |  |
| drivers/timer/hpet.rs | `heal_report` | 156 | 36 |  |
| drivers/timer/hpet.rs | `validated_read_base` | 192 | 38 |  |
| drivers/timer/hpet.rs | `guard_armed` | 230 | 13 |  |
| drivers/timer/hpet.rs | `ready` | 243 | 8 | yes |
| drivers/timer/hpet.rs | `r32` | 251 | 10 |  |
| drivers/timer/hpet.rs | `w32` | 261 | 13 |  |
| drivers/timer/hpet.rs | `read_counter` | 274 | 33 | yes |
| drivers/timer/hpet.rs | `read_time_nominal` | 307 | 71 | yes |
| drivers/timer/hpet.rs | `init` | 378 | 131 | yes |
| drivers/timer/hpet.rs | `calibrate_tsc` | 509 | 55 |  |
| drivers/timer/hpet.rs | `read_counter_raw` | 564 | 14 |  |
| drivers/timer/riscv64.rs | `get_jiffies` | 47 | 11 | yes |
| drivers/timer/riscv64.rs | `test_advance_jiffies` | 58 | 11 | yes |
| drivers/timer/riscv64.rs | `jiffy_floor_of_time` | 69 | 17 | yes |
| drivers/timer/riscv64.rs | `coarse_ns_since_boot` | 86 | 13 | yes |
| drivers/timer/riscv64.rs | `increment_jiffies` | 99 | 7 |  |
| drivers/timer/riscv64.rs | `jiffies_to_msecs` | 106 | 7 | yes |
| drivers/timer/riscv64.rs | `msecs_to_jiffies` | 113 | 6 | yes |
| drivers/timer/riscv64.rs | `read_time` | 119 | 11 | yes |
| drivers/timer/riscv64.rs | `monotonic_ns_since_boot` | 130 | 8 | yes |
| drivers/timer/riscv64.rs | `set_timer_stimecmp` | 138 | 12 |  |
| drivers/timer/riscv64.rs | `set_timer_sbi` | 150 | 9 |  |
| drivers/timer/riscv64.rs | `set_timer` | 159 | 13 |  |
| drivers/timer/riscv64.rs | `set_next_trigger` | 172 | 13 | yes |
| drivers/timer/riscv64.rs | `rearm_for_hres` | 185 | 19 |  |
| drivers/timer/riscv64.rs | `timer_interrupt_handler` | 204 | 50 | yes |
| drivers/timer/x86_64.rs | `jiffy_floor_of_time` | 133 | 7 | yes |
| drivers/timer/x86_64.rs | `coarse_ns_since_boot` | 140 | 10 | yes |
| drivers/timer/x86_64.rs | `monotonic_ns_since_boot` | 150 | 35 | yes |
| drivers/timer/x86_64.rs | `read_time` | 185 | 69 | yes |
| drivers/timer/x86_64.rs | `promote_tsc_timekeeper` | 254 | 19 |  |
| drivers/timer/x86_64.rs | `resync_tsc_against_hpet` | 273 | 60 | yes |
| drivers/timer/x86_64.rs | `calibrate_cpu_tsc_offset` | 333 | 40 | yes |
| drivers/timer/x86_64.rs | `resync_report` | 373 | 13 |  |
| drivers/timer/x86_64.rs | `tsc_active` | 386 | 10 |  |
| drivers/timer/x86_64.rs | `lapic_time_calibrated` | 396 | 22 |  |
| drivers/timer/x86_64.rs | `init` | 418 | 16 | yes |
| drivers/timer/x86_64.rs | `set_timer` | 434 | 20 |  |
| drivers/timer/x86_64.rs | `set_next_trigger` | 454 | 28 | yes |
| drivers/timer/x86_64.rs | `rearm_for_hres` | 482 | 14 |  |
| drivers/timer/x86_64.rs | `timebase_precise` | 496 | 6 |  |
| drivers/timer/x86_64.rs | `pre_hpet_time` | 502 | 12 |  |
| drivers/timer/x86_64.rs | `increment_jiffies_from_timebase` | 514 | 8 |  |
| drivers/timer/x86_64.rs | `timer_interrupt_handler` | 522 | 68 | yes |
| drivers/virtio/mod.rs | `new` | 94 | 13 | yes |
| drivers/virtio/mod.rs | `init` | 107 | 208 | yes |
| drivers/virtio/mod.rs | `get_capacity` | 315 | 7 |  |
| drivers/virtio/mod.rs | `handle_request` | 322 | 39 |  |
| drivers/virtio/mod.rs | `read_block` | 361 | 181 |  |
| drivers/virtio/mod.rs | `write_block` | 542 | 178 |  |
| drivers/virtio/mod.rs | `virtio_blk_kick_op` | 720 | 29 |  |
| drivers/virtio/mod.rs | `submit_read_async` | 749 | 136 |  |
| drivers/virtio/mod.rs | `submit_write_async` | 885 | 120 |  |
| drivers/virtio/mod.rs | `async_read_fn` | 1005 | 17 |  |
| drivers/virtio/mod.rs | `async_write_fn` | 1022 | 74 |  |
| drivers/virtio/mod.rs | `io_buf_alloc` | 1096 | 21 |  |
| drivers/virtio/mod.rs | `io_buf_free` | 1117 | 22 |  |
| drivers/virtio/mod.rs | `io_buf_init` | 1139 | 25 |  |
| drivers/virtio/mod.rs | `io_buf_release` | 1164 | 19 |  |
| drivers/virtio/mod.rs | `begin_delivery` | 1183 | 6 |  |
| drivers/virtio/mod.rs | `deliver_completion` | 1189 | 83 |  |
| drivers/virtio/mod.rs | `pci_blk_acquire_site` | 1272 | 23 |  |
| drivers/virtio/mod.rs | `wedge_put` | 1295 | 8 |  |
| drivers/virtio/mod.rs | `wedge_hex` | 1303 | 19 |  |
| drivers/virtio/mod.rs | `wedge_dec` | 1322 | 21 |  |
| drivers/virtio/mod.rs | `pci_blk_contended_forensics` | 1343 | 79 |  |
| drivers/virtio/mod.rs | `arm` | 1422 | 26 |  |
| drivers/virtio/mod.rs | `drop` | 1448 | 13 | yes |
| drivers/virtio/mod.rs | `pci_blk_lock` | 1461 | 39 |  |
| drivers/virtio/mod.rs | `pci_blk_lock_defer` | 1500 | 20 |  |
| drivers/virtio/mod.rs | `pci_blk_holder_watchdog` | 1520 | 43 |  |
| drivers/virtio/probe.rs | `virtio_mmio_va` | 56 | 14 |  |
| drivers/virtio/probe.rs | `virtio_probe_devices` | 70 | 70 |  |
| drivers/virtio/probe.rs | `init_virtio_net` | 140 | 17 |  |
| drivers/virtio/probe.rs | `init_virtio_blk` | 157 | 31 |  |
| drivers/virtio/probe.rs | `init_loopback_device` | 188 | 19 |  |
| drivers/virtio/probe.rs | `init_network_devices` | 207 | 32 |  |
| drivers/virtio/probe.rs | `init_pci_net_devices` | 239 | 56 |  |
| drivers/virtio/probe.rs | `register_probe_initcalls` | 295 | 22 |  |
| drivers/virtio/probe.rs | `init_block_devices_actual` | 317 | 4 |  |
| drivers/virtio/probe.rs | `init_block_devices` | 321 | 25 |  |
| drivers/virtio/probe.rs | `init_pci_block_devices` | 346 | 281 |  |
| drivers/virtio/probe.rs | `pci_dev_hierarchy` | 627 | 46 |  |
| drivers/virtio/probe.rs | `pci_rescan_block_hotplug` | 673 | 103 |  |
| drivers/virtio/queue.rs | `used_ring_idx` | 156 | 14 |  |
| drivers/virtio/queue.rs | `used_ring_elem_id` | 170 | 16 |  |
| drivers/virtio/queue.rs | `resp_done` | 186 | 6 |  |
| drivers/virtio/queue.rs | `resp_status_byte` | 192 | 34 |  |
| drivers/virtio/queue.rs | `new` | 226 | 5 | yes |
| drivers/virtio/queue.rs | `with_notify_width` | 231 | 113 |  |
| drivers/virtio/queue.rs | `free_vring` | 344 | 11 |  |
| drivers/virtio/queue.rs | `get_avail` | 355 | 10 |  |
| drivers/virtio/queue.rs | `get_used` | 365 | 15 |  |
| drivers/virtio/queue.rs | `get_used_elem` | 380 | 8 |  |
| drivers/virtio/queue.rs | `get_last_used` | 388 | 11 |  |
| drivers/virtio/queue.rs | `notify` | 399 | 71 | yes |
| drivers/virtio/queue.rs | `wait_for_completion` | 470 | 13 |  |
| drivers/virtio/queue.rs | `wait_for_completion_max` | 483 | 29 |  |
| drivers/virtio/queue.rs | `used_ring_ptr` | 512 | 9 |  |
| drivers/virtio/queue.rs | `avail_shadow_snapshot` | 521 | 20 |  |
| drivers/virtio/queue.rs | `vw_pollstall_print` | 541 | 8 |  |
| drivers/virtio/queue.rs | `pdec` | 549 | 95 | yes |
| drivers/virtio/queue.rs | `wait_for_used_interruptible` | 644 | 401 |  |
| drivers/virtio/queue.rs | `wait_for_desc_completion` | 1045 | 100 |  |
| drivers/virtio/queue.rs | `submit_quiet` | 1145 | 31 |  |
| drivers/virtio/queue.rs | `submit` | 1176 | 11 | yes |
| drivers/virtio/queue.rs | `get_desc` | 1187 | 9 |  |
| drivers/virtio/queue.rs | `alloc_desc` | 1196 | 13 |  |
| drivers/virtio/queue.rs | `alloc_desc_chain` | 1209 | 17 |  |
| drivers/virtio/queue.rs | `alloc_desc_run_checked` | 1226 | 61 |  |
| drivers/virtio/queue.rs | `alloc_desc_run` | 1287 | 10 |  |
| drivers/virtio/queue.rs | `hold_descs` | 1297 | 7 |  |
| drivers/virtio/queue.rs | `release_descs` | 1304 | 13 |  |
| drivers/virtio/queue.rs | `descs_held_snapshot` | 1317 | 10 |  |
| drivers/virtio/queue.rs | `reclaim_descs` | 1327 | 19 |  |
| drivers/virtio/queue.rs | `note_timed_out_chain` | 1346 | 25 |  |
| drivers/virtio/virtio_pci.rs | `device_alias_oob_warned` | 19 | 97 |  |
| drivers/virtio/virtio_pci.rs | `common_cfg` | 116 | 11 |  |
| drivers/virtio/virtio_pci.rs | `find_virtio_capability` | 127 | 48 |  |
| drivers/virtio/virtio_pci.rs | `read_virtio_cap` | 175 | 31 |  |
| drivers/virtio/virtio_pci.rs | `new` | 206 | 254 | yes |
| drivers/virtio/virtio_pci.rs | `intx_ack_only_service` | 460 | 5 |  |
