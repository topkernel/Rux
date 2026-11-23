# Core-size worklist (generated — do not edit by hand)

`hot` = called by name from a hot-path file (see scripts/core_size_map.py).
Hot functions STAY in the core: a hot site calls them directly and the
compiler can inline them. Every other row LEAVES the core for the
subsystem's service crate. The test over-keeps on purpose; a cold row
that turns out to be on a path the scan misses stays, and its lane
records why.

# security — hot 27 lines (stay) / cold 355 lines (leave)

| file | fn | line | lines | hot |
|---|---|---|---|---|
| security/cap_lsm.rs | `name` | 19 | 5 |  |
| security/cap_lsm.rs | `order` | 23 | 5 |  |
| security/cap_lsm.rs | `call` | 27 | 47 |  |
| security/capability.rs | `new` | 120 | 8 |  |
| security/capability.rs | `has` | 127 | 10 |  |
| security/capability.rs | `set` | 136 | 9 |  |
| security/capability.rs | `clear` | 144 | 9 |  |
| security/capability.rs | `intersect` | 152 | 7 |  |
| security/capability.rs | `union` | 158 | 7 |  |
| security/capability.rs | `xor` | 164 | 7 |  |
| security/capability.rs | `complement` | 170 | 7 |  |
| security/capability.rs | `is_subset_of` | 176 | 7 |  |
| security/capability.rs | `is_empty` | 182 | 7 |  |
| security/capability.rs | `bits` | 188 | 7 |  |
| security/capability.rs | `lo` | 194 | 7 |  |
| security/capability.rs | `hi` | 200 | 7 |  |
| security/capability.rs | `from_halves` | 206 | 5 |  |
| security/lsm.rs | `new` | 61 | 17 |  |
| security/lsm.rs | `default` | 77 | 9 |  |
| security/lsm.rs | `name` | 85 | 5 |  |
| security/lsm.rs | `order` | 89 | 9 |  |
| security/lsm.rs | `call` | 97 | 19 |  |
| security/lsm.rs | `register_lsm` | 115 | 29 |  |
| security/lsm.rs | `chain_is_capability_only` | 143 | 11 |  |
| security/lsm.rs | `security_hook_call` | 153 | 18 |  |
| security/lsm.rs | `security_capable` | 170 | 11 |  |
| security/lsm.rs | `security_signal_send` | 180 | 14 |  |
| security/lsm.rs | `lsm_count` | 193 | 4 |  |
| security/mod.rs | `capable` | 38 | 16 | yes |
| security/mod.rs | `has_capability` | 53 | 11 | yes |
| security/mod.rs | `can_send_signal` | 63 | 29 |  |
| security/mod.rs | `security_init` | 91 | 19 |  |
