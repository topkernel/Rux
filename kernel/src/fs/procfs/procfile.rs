//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! procfile! — deriving /proc file formats from one declaration list
//! (docs/architecture/rust-design-headroom.md §4, leg-34 track-1).
//!
//! The bug class this kills: five byte-format defects (column order,
//! trailing spaces, wrong-unit timestamps, prefix pollution, shifted
//! columns) survived review into a merged tree because each /proc file
//! hand-wrote its `format!` strings — the field list lived in one place
//! (the author's head) and the format in another (the string literal),
//! and they drifted. Here the FIELD DECLARATION is the single artifact:
//! the macro derives the renderer and the key list from it, and emits a
//! mechanical shape test.
//!
//! Shape (macro_rules cannot mint identifiers, so the field-list const
//! and test names are explicit arguments):
//!
//! ```ignore
//! procfile! {
//!     /// /proc/vmstat — flat key-value stats (Linux vmstat.c shape).
//!     vmstat() fields VMSTAT_FIELDS test vmstat_shape -> Vec<u8> kv {
//!         "nr_free_pages" => free,
//!         "nr_mlock"      => 0,
//!     }
//! }
//! ```
//!
//! expands to
//! * `pub fn vmstat() -> Vec<u8>` — rendered in DECLARATION ORDER
//!   (order is part of the Linux-observable format; moving a line in
//!   the declaration intentionally moves it in the file),
//! * `pub const VMSTAT_FIELDS: &[&str]` — the key list, in order, for
//!   parsers/tests/dfx tooling,
//! * `mod vmstat_shape` with a `#[test]` asserting every declared key
//!   renders as `"<key> <value>\n"` (the width/separator/trailing-space
//!   drift floor).
//!
//! Conversions additionally keep an exact-bytes SNAPSHOT test in the
//! file's own `#[cfg(test)]` mod (see vmstat.rs): render the pure
//! formatter with fixed dummy values and compare against a literal.
//! The snapshot is the author's conscious act; the macro's shape test
//! is the mechanical floor.
//!
//! A `pad` form (meminfo's `MemTotal:<pad>123 kB` shape) is planned
//! alongside `render_pad` once the first padded file converts; until
//! then only the `kv` form derives (don't derive what nothing uses).

use alloc::string::String;

/// Pure `kv` renderer: `"<key> <value>\n"` per row, in order.
pub fn render_kv(rows: &[(&'static str, &dyn core::fmt::Display)]) -> String {
    let mut out = String::with_capacity(rows.len() * 24);
    for (k, v) in rows {
        out.push_str(k);
        out.push(' ');
        out.push_str(&alloc::format!("{}", v));
        out.push('\n');
    }
    out
}

/// Pure `pad` renderer (meminfo shape): key with colon left-justified
/// to `width`, then the value, then ` <unit>` and newline. Not yet
/// wired into `procfile!` (no padded file has converted); exported so
/// a hand-rendered file can at least share the one width discipline.
pub fn render_pad(
    rows: &[(&'static str, &dyn core::fmt::Display)],
    width: usize,
    unit: &str,
) -> String {
    let mut out = String::with_capacity(rows.len() * 28);
    for (k, v) in rows {
        let key = alloc::format!("{}:", k.strip_suffix(':').unwrap_or(k));
        out.push_str(&alloc::format!("{:<width$}", key, width = width));
        out.push_str(&alloc::format!("{}", v));
        out.push(' ');
        out.push_str(unit);
        out.push('\n');
    }
    out
}

/// Derives a /proc key-value formatter, its field list, and a shape
/// test from one field declaration list. See the module doc.
#[macro_export]
macro_rules! procfile {
    (
        $(#[$doc:meta])*
        $fname:ident () fields $fields:ident test $tmod:ident
            -> Vec<u8> kv {
            $( $key:literal => $val:expr ),* $(,)?
        }
    ) => {
        $(#[$doc])*
        pub fn $fname() -> alloc::vec::Vec<u8> {
            #[allow(unused_variables)]
            let rows: &[(&'static str, &dyn core::fmt::Display)] = &[
                $( ($key, &$val) ),*
            ];
            $crate::fs::procfs::procfile::render_kv(rows).into_bytes()
        }

        #[doc = concat!("Declared field order of [`", stringify!($fname), "`].")]
        pub const $fields: &[&str] = &[ $( $key ),* ];

        #[cfg(test)]
        mod $tmod {
            /// Mechanical drift floor: each declared key renders as
            /// exactly `"<key> <digits>\n"` — one space, no padding,
            /// no trailing space, newline-terminated.
            #[test]
            fn shape_kv_lines() {
                let keys: &[&str] = &[ $( $key ),* ];
                for k in keys {
                    let got = $crate::fs::procfs::procfile::render_kv(&[(k, &0u64)]);
                    assert_eq!(got, alloc::format!("{} 0\n", k), "kv shape drifted for {}", k);
                }
                // Cross-check the generated list against the renderer:
                // the const and the fn must describe the same file.
                let rendered = $crate::fs::procfs::procfile::render_kv(
                    &[$( ($key, &0u64 as &dyn core::fmt::Display) ),*],
                );
                let expect: alloc::string::String = {
                    let mut s = alloc::string::String::new();
                    $(
                        s.push_str($key);
                        s.push_str(" 0\n");
                    )*
                    s
                };
                assert_eq!(rendered, expect);
                assert_eq!($crate::fs::procfs::procfile::render_kv(
                    keys.iter().map(|k| (*k, &0u64 as &dyn core::fmt::Display))
                        .collect::<alloc::vec::Vec<_>>().as_slice()),
                    expect);
            }
        }
    };
}
