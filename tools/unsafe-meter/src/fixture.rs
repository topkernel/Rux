//! Self-test fixture: known-content file checked into the repo, embedded at
//! compile time and asserted by both `cargo test` (tests/selftest.rs) and the
//! `--selftest` CLI flag. One source of truth for expected values.

use std::path::Path;

use crate::{line_offsets, scan_str};

pub const FIXTURE: &str = include_str!("../tests/fixtures/fixture_input.rs");

/// Expected values, with the reasoning for each.
pub fn expected() -> Vec<(&'static str, usize)> {
    vec![
        ("total_lines", 51),
        ("block_count", 3),   // A (16), B (18, inner), C (48)
        ("block_lines", 8),   // A: 16-22 (outermost, covers inner B's lines) + C: 48
        ("fnimpl_count", 4),  // unsafe fn plain, unsafe trait, unsafe impl, unsafe extern
        ("fnimpl_lines", 12), // 11-13, 25-27, 29-31, 37-39
        ("unsafe_lines", 20),
        ("macro_tokens", 1), // template of macro_rules! m at line 43
        ("macro_lines", 1),
    ]
}

pub fn check() -> Result<String, String> {
    let m = scan_str(Path::new("tests/fixtures/fixture_input.rs"), FIXTURE);
    if !m.parse_ok {
        return Err(format!("fixture failed to parse: {:?}", m.parse_error));
    }
    let actual = vec![
        ("total_lines", m.total_lines),
        ("block_count", m.block_count),
        ("block_lines", m.block_lines.len()),
        ("fnimpl_count", m.fnimpl_count),
        ("fnimpl_lines", m.fnimpl_lines.len()),
        ("unsafe_lines", m.unsafe_lines()),
        ("macro_tokens", m.macro_internal.tokens),
        ("macro_lines", m.macro_internal.lines.len()),
    ];
    let exp = expected();
    let mut bad = Vec::new();
    for ((ek, ev), (ak, av)) in exp.iter().zip(actual.iter()) {
        if ek != ak || ev != av {
            bad.push(format!("{}: expected {}, got {}", ek, ev, av));
        }
    }

    // Exact line-set assertions (catch off-by-one line mapping, wrong
    // outermost attribution, and closing-brace handling).
    let want_block: Vec<usize> = (16..=22).chain(48..=48).collect();
    let want_fnimpl: Vec<usize> = (11..=13)
        .chain(25..=27)
        .chain(29..=31)
        .chain(37..=39)
        .collect();
    let got_block: Vec<usize> = m.block_lines.iter().copied().collect();
    let got_fnimpl: Vec<usize> = m.fnimpl_lines.iter().copied().collect();
    if got_block != want_block {
        bad.push(format!(
            "block line set: expected {:?}, got {:?}",
            want_block, got_block
        ));
    }
    if got_fnimpl != want_fnimpl {
        bad.push(format!(
            "fnimpl line set: expected {:?}, got {:?}",
            want_fnimpl, got_fnimpl
        ));
    }
    if m.macro_internal.lines.iter().copied().collect::<Vec<_>>() != vec![43] {
        bad.push(format!(
            "macro line set: expected [43], got {:?}",
            m.macro_internal.lines
        ));
    }
    let offs = line_offsets(FIXTURE);
    if offs.len() != 51 {
        bad.push(format!(
            "line_offsets: expected 51 entries, got {}",
            offs.len()
        ));
    }

    if bad.is_empty() {
        Ok(format!(
            "fixture counts match: {} blocks / {} fn+impl items / {} unsafe lines / {} macro tokens",
            m.block_count,
            m.fnimpl_count,
            m.unsafe_lines(),
            m.macro_internal.tokens
        ))
    } else {
        Err(bad.join("; "))
    }
}
