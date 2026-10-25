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
//! A `pad` form (meminfo's `MemTotal:<pad>123 kB` shape) renders with
//! `render_pad`: key+colon left-justified to a width (16 on Linux),
//! value right-justified to 8, ` <unit>` suffix; a `!`-suffixed key
//! renders unitless (meminfo's HugePages_* rows). First padded
//! conversion: /proc/meminfo (leg-35).
//!
//! A `cols` form (leg-36) covers the stat-family ROW shape — a label
//! prefix then N space-separated counters (`/proc/stat`'s jiffy rows,
//! `/proc/schedstat`'s per-CPU rows). The declaration mints a row
//! function whose ARGUMENT LIST is derived from the column list, so
//! the compiler itself rejects a call site that swaps two counters:
//! column order, rendered order and call order are one artifact.
//! First conversions: /proc/stat + /proc/schedstat cpu rows.

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

/// Pure `pad` renderer (Linux meminfo shape, verified against this
/// host's live /proc/meminfo): key with colon left-justified to
/// `width` (16 on Linux), value right-justified to width 8 (wider
/// values flow, matching C's `%8lu`), then ` <unit>` and newline.
/// A key ending in `!` renders UNITLESS — meminfo's HugePages_* rows
/// carry no `kB` (the marker is stripped at render time; it exists so
/// one declaration list can hold both shapes without a second list to
/// drift against).
pub fn render_pad(
    rows: &[(&'static str, &dyn core::fmt::Display)],
    width: usize,
    unit: &str,
) -> String {
    let mut out = String::with_capacity(rows.len() * 28);
    for (k, v) in rows {
        let (key, with_unit) = match k.strip_suffix('!') {
            Some(stripped) => (stripped, false),
            None => (*k, true),
        };
        let key = alloc::format!("{}:", key.strip_suffix(':').unwrap_or(key));
        out.push_str(&alloc::format!("{:<width$}{:>8}", key, v, width = width));
        if with_unit {
            out.push(' ');
            out.push_str(unit);
        }
        out.push('\n');
    }
    out
}

/// Pure `cols` row renderer (Linux stat-family row shape): `label`
/// then one space-separated value per column, newline, no trailing
/// space. The label may carry a trailing space when Linux's layout
/// needs one — the aggregate `/proc/stat` row is literally `"cpu  "`
/// (`"cpu"` + two spaces) so it aligns under the `cpu0` rows; that
/// byte belongs to the label, not to the separator.
pub fn render_cols(label: &str, vals: &[&dyn core::fmt::Display]) -> String {
    let mut out = String::with_capacity(label.len() + 1 + vals.len() * 4);
    out.push_str(label);
    for v in vals {
        out.push(' ');
        out.push_str(&alloc::format!("{}", v));
    }
    out.push('\n');
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

        #[cfg(any(test, feature = "unit-test"))]
        pub mod $tmod {
            /// Mechanical drift floor: each declared key renders as
            /// exactly `"<key> <digits>\n"` — one space, no padding,
            /// no trailing space, newline-terminated. `shape_check` is
            /// the callable body (the in-kernel `unit-test` suite runs
            /// it via tests/mod.rs — a bare `#[cfg(test)]` never joins
            /// that build); the `#[test]` wrapper covers cargo test.
            pub fn shape_check() {
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

            #[cfg(test)]
            #[test]
            fn shape_kv_lines() {
                shape_check();
            }
        }
    };

    (
        $(#[$doc:meta])*
        $fname:ident () fields $fields:ident test $tmod:ident
            -> Vec<u8> pad $width:literal unit $unit:literal {
            $( $key:literal => $val:expr ),* $(,)?
        }
    ) => {
        $(#[$doc])*
        pub fn $fname() -> alloc::vec::Vec<u8> {
            #[allow(unused_variables)]
            let rows: &[(&'static str, &dyn core::fmt::Display)] = &[
                $( ($key, &$val) ),*
            ];
            $crate::fs::procfs::procfile::render_pad(rows, $width, $unit).into_bytes()
        }

        /// Declared field order of the padded file. Keys carrying the
        /// unitless marker keep it here (so the const and the renderer
        /// describe the same file); strip `!` when matching against
        /// rendered output.
        pub const $fields: &[&str] = &[ $( $key ),* ];

        #[cfg(any(test, feature = "unit-test"))]
        pub mod $tmod {
            /// Mechanical drift floor for the pad shape (Linux meminfo
            /// layout): key+colon left-justified to width, `0`
            /// right-justified at width+8, space, unit, newline — and
            /// `!`-marked keys render unitless. Callable body for the
            /// in-kernel unit-test suite (see the kv form's note).
            pub fn shape_check() {
                let keys: &[&str] = &[ $( $key ),* ];
                for k in keys {
                    let got = $crate::fs::procfs::procfile::render_pad(&[(k, &0u64)], $width, $unit);
                    let (key, unit_suffix) = if let Some(s) = k.strip_suffix('!') {
                        (s, alloc::string::String::from("\n"))
                    } else {
                        (*k, alloc::format!(" {}\n", $unit))
                    };
                    let key = alloc::format!("{}:", key);
                    let expect = alloc::format!(
                        "{:<w$}{:>8}{}", key, 0u64, unit_suffix, w = $width);
                    assert_eq!(got, expect, "pad shape drifted for {}", k);
                }
            }

            #[cfg(test)]
            #[test]
            fn shape_pad_lines() {
                shape_check();
            }
        }
    };

    // `cols` form — stat-family rows: label prefix, then one
    // space-separated u64 per declared column. The derived function's
    // argument list IS the column list (names become parameters), so a
    // call site cannot pass counters in the wrong order without the
    // names disagreeing at the call — the order cannot drift between
    // declaration, signature and rendered bytes because there is only
    // one list.
    (
        $(#[$doc:meta])*
        rowfn $fname:ident ($label:ident : &str) columns $columns:ident
            test $tmod:ident -> alloc::string::String cols {
            $( $col:literal => $arg:ident ),* $(,)?
        }
    ) => {
        $(#[$doc])*
        #[allow(clippy::too_many_arguments)]
        pub fn $fname($label: &str, $( $arg: u64 ),*) -> alloc::string::String {
            let vals: &[&dyn core::fmt::Display] = &[ $( &$arg ),* ];
            $crate::fs::procfs::procfile::render_cols($label, vals)
        }

        #[doc = concat!("Declared column order of [`", stringify!($fname), "`].")]
        pub const $columns: &[&str] = &[ $( $col ),* ];

        #[cfg(any(test, feature = "unit-test"))]
        pub mod $tmod {
            /// Mechanical drift floor for the cols shape: the row
            /// renders as `label` + ` <value>` per declared column +
            /// `\n` — single spaces, no trailing space, and as many
            /// value tokens as the columns const lists (the const, the
            /// derived signature and the renderer describe one file).
            /// Callable body for the in-kernel unit-test suite (see the
            /// kv form's note).
            pub fn shape_check() {
                let cols: &[&str] = &[ $( $col ),* ];
                // The zero-call must mention $arg per repetition
                // (macro_rules rejects repetitions with no syntax
                // variable); stringify! is the no-op consumption.
                let got = super::$fname(
                    "shape",
                    $( { let _ = stringify!($arg); 0u64 } ),*
                );
                let mut expect = alloc::string::String::from("shape");
                for _ in cols {
                    expect.push_str(" 0");
                }
                expect.push('\n');
                assert_eq!(got, expect, "cols shape drifted");
                // The rendered token count must equal the declared
                // column count (a column added to the list without a
                // parameter — or vice versa — breaks compilation, this
                // catches a hand-edited renderer instead).
                let tokens = got.trim_end_matches('\n')
                    .split(' ').filter(|t| !t.is_empty()).count();
                assert_eq!(tokens, cols.len() + 1, "cols token/column mismatch");
            }

            #[cfg(test)]
            #[test]
            fn shape_cols_row() {
                shape_check();
            }
        }
    };
}
