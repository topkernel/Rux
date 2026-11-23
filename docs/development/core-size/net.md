# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# net — hot 93 lines (stay) / cold 712 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| net/buffer.rs | `from_u16` | 44 | 11 |  |
| net/buffer.rs | `to_u16` | 55 | 24 |  |
| net/buffer.rs | `from_u8` | 79 | 12 |  |
| net/buffer.rs | `to_u8` | 91 | 47 |  |
| net/buffer.rs | `drop` | 138 | 21 | yes |
| net/buffer.rs | `alloc` | 159 | 52 | yes |
| net/buffer.rs | `free` | 211 | 5 | yes |
| net/buffer.rs | `release` | 216 | 36 |  |
| net/buffer.rs | `skb_put` | 252 | 24 |  |
| net/buffer.rs | `skb_push` | 276 | 23 |  |
| net/buffer.rs | `skb_pull` | 299 | 26 |  |
| net/buffer.rs | `skb_reserve` | 325 | 22 |  |
| net/buffer.rs | `skb_put_data` | 347 | 16 |  |
| net/buffer.rs | `set_mac_header` | 363 | 9 |  |
| net/buffer.rs | `set_network_header` | 372 | 8 |  |
| net/buffer.rs | `set_transport_header` | 380 | 5 |  |
| net/buffer.rs | `get_mac_header` | 385 | 5 |  |
| net/buffer.rs | `get_network_header` | 390 | 5 |  |
| net/buffer.rs | `get_transport_header` | 395 | 5 |  |
| net/buffer.rs | `data` | 400 | 5 |  |
| net/buffer.rs | `data_mut` | 405 | 5 |  |
| net/buffer.rs | `len` | 410 | 5 | yes |
| net/buffer.rs | `is_empty` | 415 | 8 | yes |
| net/buffer.rs | `data_slice` | 423 | 9 |  |
| net/buffer.rs | `data_slice_at` | 432 | 13 |  |
| net/buffer.rs | `hdr` | 445 | 12 |  |
| net/buffer.rs | `hdr_mut` | 457 | 14 |  |
| net/buffer.rs | `hdr_and_tail` | 471 | 26 |  |
| net/buffer.rs | `push_hdr` | 497 | 12 |  |
| net/buffer.rs | `put_hdr` | 509 | 11 |  |
| net/buffer.rs | `push_bytes` | 520 | 10 |  |
| net/buffer.rs | `push_hdr_val` | 530 | 23 |  |
| net/buffer.rs | `skb_copy_bits` | 553 | 27 |  |
| net/buffer.rs | `alloc_skb` | 580 | 8 |  |
| net/buffer.rs | `kfree_skb` | 588 | 10 |  |
| net/buffer.rs | `data_all` | 598 | 12 |  |
| net/buffer.rs | `hdr_view` | 610 | 11 |  |
| net/buffer.rs | `hdr_view_mut` | 621 | 7 |  |
| net/file_arc.rs | `file_arc` | 9 | 15 |  |
| net/file_arc.rs | `take_file_arc` | 24 | 8 |  |
| net/mod.rs | `file_arc` | 45 | 16 |  |
| net/mod.rs | `reclaim_file_arc` | 61 | 11 |  |
| net/mod.rs | `take_file_arc` | 72 | 10 |  |
| net/net_edges.rs | `static_table_ref` | 22 | 7 |  |
| net/net_edges.rs | `static_table_mut` | 29 | 11 |  |
| net/net_edges.rs | `arc_refcount_probe` | 40 | 14 |  |
| net/net_edges.rs | `audited_deref` | 54 | 10 |  |
| net/net_edges.rs | `container_of` | 64 | 9 |  |
| net/net_edges.rs | `pod_view` | 73 | 18 |  |
| net/net_edges.rs | `pod_view_mut` | 91 | 8 |  |
| net/net_hooks.rs | `net_register_hooks` | 181 | 5 |  |
| net/net_hooks.rs | `hooks` | 186 | 7 |  |
| net/net_hooks.rs | `tcp_rows` | 193 | 3 |  |
| net/net_hooks.rs | `tcp_rows_dump` | 196 | 2 |  |
| net/net_hooks.rs | `udp_rows` | 198 | 2 |  |
| net/net_hooks.rs | `udp_rows_dump` | 200 | 2 |  |
| net/net_hooks.rs | `unix_registry_snapshot` | 202 | 4 |  |
| net/net_hooks.rs | `unix_net_snapshot` | 206 | 2 |  |
| net/net_hooks.rs | `arp_rows` | 208 | 2 |  |
| net/net_hooks.rs | `arp_rows_dump` | 210 | 2 |  |
| net/net_hooks.rs | `route_rows` | 212 | 2 |  |
| net/net_hooks.rs | `route_rows_dump` | 214 | 2 |  |
| net/net_hooks.rs | `dev_rows` | 216 | 2 |  |
| net/net_hooks.rs | `is_unix_socket_ops` | 218 | 2 |  |
| net/net_hooks.rs | `is_inet_socket_ops` | 220 | 2 |  |
| net/net_hooks.rs | `is_netlink_ops` | 222 | 2 |  |
| net/net_hooks.rs | `is_raw_ops` | 224 | 2 |  |
| net/net_hooks.rs | `unix_socket_of` | 226 | 2 |  |
| net/net_hooks.rs | `mq_notify_sock` | 228 | 3 |  |
| net/net_hooks.rs | `unix_fd_bound_peer` | 231 | 2 |  |
| net/net_hooks.rs | `unix_file_diag` | 233 | 2 |  |
| net/net_hooks.rs | `unix_recv_describe` | 235 | 2 |  |
| net/net_hooks.rs | `unix_listener_snapshot` | 237 | 2 |  |
| net/net_hooks.rs | `unix_park_audit` | 239 | 2 |  |
| net/net_hooks.rs | `tcp_table_nonempty` | 241 | 2 |  |
| net/net_hooks.rs | `tcp_census_rows` | 243 | 2 |  |
| net/net_hooks.rs | `ethernet_poll` | 245 | 2 |  |
| net/net_hooks.rs | `tcp_timer_softirq` | 247 | 2 |  |
| net/net_hooks.rs | `uevent_broadcast` | 249 | 2 |  |
| net/net_hooks.rs | `socket_file_stat` | 251 | 2 | yes |
| net/net_hooks.rs | `netlink_push_kernel_message` | 253 | 3 |  |
| net/net_hooks.rs | `init_tcp` | 256 | 2 |  |
| net/net_hooks.rs | `init_tcp_timer` | 258 | 2 |  |
| net/net_hooks.rs | `init_route` | 260 | 2 |  |
| net/net_hooks.rs | `set_local_ip` | 262 | 2 |  |
| net/net_hooks.rs | `ipv6_init` | 264 | 1 |  |
