# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# net — hot 0 lines (stay) / cold 891 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| net/buffer.rs | `from_u16` | 44 | 12 |  |
| net/buffer.rs | `to_u16` | 55 | 25 |  |
| net/buffer.rs | `from_u8` | 79 | 13 |  |
| net/buffer.rs | `to_u8` | 91 | 48 |  |
| net/buffer.rs | `drop` | 138 | 22 |  |
| net/buffer.rs | `alloc` | 159 | 53 |  |
| net/buffer.rs | `free` | 211 | 6 |  |
| net/buffer.rs | `release` | 216 | 37 |  |
| net/buffer.rs | `skb_put` | 252 | 25 |  |
| net/buffer.rs | `skb_push` | 276 | 24 |  |
| net/buffer.rs | `skb_pull` | 299 | 27 |  |
| net/buffer.rs | `skb_reserve` | 325 | 23 |  |
| net/buffer.rs | `skb_put_data` | 347 | 17 |  |
| net/buffer.rs | `set_mac_header` | 363 | 10 |  |
| net/buffer.rs | `set_network_header` | 372 | 9 |  |
| net/buffer.rs | `set_transport_header` | 380 | 6 |  |
| net/buffer.rs | `get_mac_header` | 385 | 6 |  |
| net/buffer.rs | `get_network_header` | 390 | 6 |  |
| net/buffer.rs | `get_transport_header` | 395 | 6 |  |
| net/buffer.rs | `data` | 400 | 6 |  |
| net/buffer.rs | `data_mut` | 405 | 6 |  |
| net/buffer.rs | `len` | 410 | 6 |  |
| net/buffer.rs | `is_empty` | 415 | 9 |  |
| net/buffer.rs | `data_slice` | 423 | 10 |  |
| net/buffer.rs | `data_slice_at` | 432 | 14 |  |
| net/buffer.rs | `hdr` | 445 | 13 |  |
| net/buffer.rs | `hdr_mut` | 457 | 15 |  |
| net/buffer.rs | `hdr_and_tail` | 471 | 27 |  |
| net/buffer.rs | `push_hdr` | 497 | 13 |  |
| net/buffer.rs | `put_hdr` | 509 | 12 |  |
| net/buffer.rs | `push_bytes` | 520 | 11 |  |
| net/buffer.rs | `push_hdr_val` | 530 | 24 |  |
| net/buffer.rs | `skb_copy_bits` | 553 | 28 |  |
| net/buffer.rs | `alloc_skb` | 580 | 9 |  |
| net/buffer.rs | `kfree_skb` | 588 | 11 |  |
| net/buffer.rs | `data_all` | 598 | 13 |  |
| net/buffer.rs | `hdr_view` | 610 | 12 |  |
| net/buffer.rs | `hdr_view_mut` | 621 | 8 |  |
| net/file_arc.rs | `file_arc` | 9 | 16 |  |
| net/file_arc.rs | `take_file_arc` | 24 | 9 |  |
| net/mod.rs | `file_arc` | 45 | 17 |  |
| net/mod.rs | `reclaim_file_arc` | 61 | 12 |  |
| net/mod.rs | `take_file_arc` | 72 | 11 |  |
| net/net_edges.rs | `static_table_ref` | 22 | 8 |  |
| net/net_edges.rs | `static_table_mut` | 29 | 12 |  |
| net/net_edges.rs | `arc_refcount_probe` | 40 | 15 |  |
| net/net_edges.rs | `audited_deref` | 54 | 11 |  |
| net/net_edges.rs | `container_of` | 64 | 10 |  |
| net/net_edges.rs | `pod_view` | 73 | 19 |  |
| net/net_edges.rs | `pod_view_mut` | 91 | 9 |  |
| net/net_hooks.rs | `net_register_hooks` | 181 | 6 |  |
| net/net_hooks.rs | `hooks` | 186 | 8 |  |
| net/net_hooks.rs | `tcp_rows` | 193 | 4 |  |
| net/net_hooks.rs | `tcp_rows_dump` | 196 | 3 |  |
| net/net_hooks.rs | `udp_rows` | 198 | 3 |  |
| net/net_hooks.rs | `udp_rows_dump` | 200 | 3 |  |
| net/net_hooks.rs | `unix_registry_snapshot` | 202 | 5 |  |
| net/net_hooks.rs | `unix_net_snapshot` | 206 | 3 |  |
| net/net_hooks.rs | `arp_rows` | 208 | 3 |  |
| net/net_hooks.rs | `arp_rows_dump` | 210 | 3 |  |
| net/net_hooks.rs | `route_rows` | 212 | 3 |  |
| net/net_hooks.rs | `route_rows_dump` | 214 | 3 |  |
| net/net_hooks.rs | `dev_rows` | 216 | 3 |  |
| net/net_hooks.rs | `is_unix_socket_ops` | 218 | 3 |  |
| net/net_hooks.rs | `is_inet_socket_ops` | 220 | 3 |  |
| net/net_hooks.rs | `is_netlink_ops` | 222 | 3 |  |
| net/net_hooks.rs | `is_raw_ops` | 224 | 3 |  |
| net/net_hooks.rs | `unix_socket_of` | 226 | 3 |  |
| net/net_hooks.rs | `mq_notify_sock` | 228 | 4 |  |
| net/net_hooks.rs | `unix_fd_bound_peer` | 231 | 3 |  |
| net/net_hooks.rs | `unix_file_diag` | 233 | 3 |  |
| net/net_hooks.rs | `unix_recv_describe` | 235 | 3 |  |
| net/net_hooks.rs | `unix_listener_snapshot` | 237 | 3 |  |
| net/net_hooks.rs | `unix_park_audit` | 239 | 3 |  |
| net/net_hooks.rs | `tcp_table_nonempty` | 241 | 3 |  |
| net/net_hooks.rs | `tcp_census_rows` | 243 | 3 |  |
| net/net_hooks.rs | `ethernet_poll` | 245 | 3 |  |
| net/net_hooks.rs | `tcp_timer_softirq` | 247 | 3 |  |
| net/net_hooks.rs | `uevent_broadcast` | 249 | 3 |  |
| net/net_hooks.rs | `socket_file_stat` | 251 | 3 |  |
| net/net_hooks.rs | `netlink_push_kernel_message` | 253 | 4 |  |
| net/net_hooks.rs | `init_tcp` | 256 | 3 |  |
| net/net_hooks.rs | `init_tcp_timer` | 258 | 3 |  |
| net/net_hooks.rs | `init_route` | 260 | 3 |  |
| net/net_hooks.rs | `set_local_ip` | 262 | 3 |  |
| net/net_hooks.rs | `ipv6_init` | 264 | 2 |  |
