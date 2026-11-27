//! Parser-grade unsafe-code enumeration for the Rux kernel.
//!
//! Replaces the naive brace-scanning meter, which mis-tracked the archive
//! branch's macro-heavy style (`asm!`, `bitflags!` consume unbalanced-looking
//! braces and poisoned the scan: 2,277 "blocks" averaging 1.15 lines).
//!
//! Methodology (matches the README dual-series accounting):
//! - **block-series**: every `unsafe { … }` block (syn `ExprUnsafe`).
//!   Lines = the block's span, attributed only if the block is OUTERMOST
//!   (no enclosing unsafe fn / impl / block).
//! - **fn/impl-series**: `unsafe fn` (items, impl methods, trait methods),
//!   `unsafe trait`, `unsafe impl`, `unsafe extern` blocks. Lines = the
//!   whole item span, attributed only if OUTERMOST.
//! - Inner unsafe regions inside an outer unsafe region still COUNT (each
//!   series' count is total), but their lines attribute to the outermost
//!   region — no double counting.
//! - **macro-internal (approximate)**: syn sees macro invocations as opaque
//!   token streams, so unsafe that lives inside a macro body is invisible to
//!   the two primary series. Best effort: scan every macro invocation /
//!   `macro_rules!` definition token stream for `unsafe` keyword tokens and
//!   report count + physical lines. Identical treatment on both branches.
//!
//! Line mapping: proc-macro2 fallback spans (`span-locations` feature),
//! byte offsets mapped through a per-file line-offset table.

use proc_macro2::{TokenStream, TokenTree};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use syn::visit::Visit;
use syn::ItemMacro;


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    /// `unsafe { … }`
    Block,
    /// `unsafe fn` / `unsafe trait` / `unsafe impl` / `unsafe extern`
    FnImpl,
}

#[derive(Debug, Clone, Copy)]
struct Region {
    kind: RegionKind,
    start: usize, // byte offset, inclusive
    end: usize,   // byte offset, inclusive
}

#[derive(Debug, Default, Clone)]
pub struct MacroUnsafe {
    /// `unsafe` keyword tokens found inside macro invocation bodies.
    pub tokens: usize,
    /// Union of physical lines covered by those tokens.
    pub lines: BTreeSet<usize>,
}

#[derive(Debug, Default, Clone)]
pub struct FileMetrics {
    pub path: PathBuf,
    /// All physical lines in the file (the %-denominator).
    pub total_lines: usize,
    /// Count of every `unsafe { }` block (outermost AND inner).
    pub block_count: usize,
    /// Physical lines attributed to the block series (outermost blocks only).
    pub block_lines: BTreeSet<usize>,
    /// Count of every unsafe fn/trait/impl/extern item.
    pub fnimpl_count: usize,
    /// Physical lines attributed to the fn/impl series (outermost only).
    pub fnimpl_lines: BTreeSet<usize>,
    /// Approximate: `unsafe` tokens inside macro bodies.
    pub macro_internal: MacroUnsafe,
    pub parse_ok: bool,
    pub parse_error: Option<String>,
}

impl FileMetrics {
    pub fn unsafe_lines(&self) -> usize {
        self.block_lines.len() + self.fnimpl_lines.len()
    }
}

/// Byte offset of the start of every 1-based line (index 0 = line 1).
/// A trailing newline does NOT create a phantom extra line.
pub fn line_offsets(src: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    if src.ends_with('\n') {
        starts.pop();
    }
    starts
}

/// 1-based physical line of a byte offset (binary search over the table).
pub fn line_of(offsets: &[usize], offset: usize) -> usize {
    match offsets.binary_search(&offset) {
        Ok(i) => i + 1,
        Err(i) => i, // offset sits inside line i (1-based)
    }
}

/// 1-based inclusive line span [start, end] of a byte region,
/// mapped through the per-file line-offset table. `end` is INCLUSIVE.
fn byte_region_lines(start: usize, end: usize, offsets: &[usize]) -> (usize, usize) {
    (line_of(offsets, start), line_of(offsets, end))
}

fn region_contains_line(r: &Region, line: usize, offsets: &[usize]) -> bool {
    let (a, b) = byte_region_lines(r.start, r.end, offsets);
    a <= line && line <= b
}

/// Full byte extent of a token stream (groups recursed into).
/// Used to sanity-check syn::spanned::Spanned extents in the self-test.
#[allow(dead_code)]
fn stream_extent(ts: &TokenStream) -> Option<(usize, usize)> {
    let mut lo = usize::MAX;
    let mut hi = 0usize;
    let mut any = false;
    for tt in ts.clone().into_iter() {
        any = true;
        let (a, b) = tree_extent(&tt);
        lo = lo.min(a);
        hi = hi.max(b);
    }
    if any { Some((lo, hi)) } else { None }
}

#[allow(dead_code)]
fn tree_extent(tt: &TokenTree) -> (usize, usize) {
    let s = tt.span().byte_range();
    match &tt {
        TokenTree::Group(g) => match stream_extent(&g.stream()) {
            Some((a, b)) => (s.start.min(a), s.end.max(b)),
            None => (s.start, s.end),
        },
        _ => (s.start, s.end),
    }
}

fn push<T: syn::spanned::Spanned>(node: &T, kind: RegionKind, regions: &mut Vec<Region>) {
    // Spanned::span() joins all token spans of the node; byte_range() on the
    // joined span gives [start, end) over the parsed source string.
    // Region.end is stored INCLUSIVE (last byte of the node).
    let range = node.span().byte_range();
    let start = range.start.min(range.end);
    let end = range.end.saturating_sub(1).max(start);
    regions.push(Region { kind, start, end });
}

/// Scan one Rust source string.
pub fn scan_str(path: &Path, src: &str) -> FileMetrics {
    let offsets = line_offsets(src);
    let mut m = FileMetrics {
        path: path.to_path_buf(),
        total_lines: src.lines().count(),
        ..Default::default()
    };

    let file = match syn::parse_file(src) {
        Ok(f) => f,
        Err(e) => {
            m.parse_ok = false;
            m.parse_error = Some(e.to_string());
            return m;
        }
    };
    m.parse_ok = true;

    let mut regions: Vec<Region> = Vec::new();
    let mut macro_internal = MacroUnsafe::default();
    let mut visitor = Visitor {
        offsets: &offsets,
        regions: &mut regions,
        macro_internal: &mut macro_internal,
    };
    visitor.visit_file(&file);

    m.block_count = regions.iter().filter(|r| r.kind == RegionKind::Block).count();
    m.fnimpl_count = regions.iter().filter(|r| r.kind == RegionKind::FnImpl).count();
    m.macro_internal = macro_internal;

    // Outermost attribution: with proper span nesting, sort by (start asc,
    // end desc) and the first region whose line span contains a line is the
    // OUTERMOST region containing that line.
    let mut sorted: Vec<&Region> = regions.iter().collect();
    sorted.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
    for line in 1..=m.total_lines {
        if let Some(r) = sorted.iter().find(|r| region_contains_line(r, line, &offsets)) {
            match r.kind {
                RegionKind::Block => {
                    m.block_lines.insert(line);
                }
                RegionKind::FnImpl => {
                    m.fnimpl_lines.insert(line);
                }
            }
        }
    }
    m
}

struct Visitor<'a> {
    offsets: &'a [usize],
    regions: &'a mut Vec<Region>,
    macro_internal: &'a mut MacroUnsafe,
}

impl<'a> Visit<'a> for Visitor<'a> {
    fn visit_expr_unsafe(&mut self, i: &'a syn::ExprUnsafe) {
        push(i, RegionKind::Block, self.regions);
        syn::visit::visit_expr_unsafe(self, i);
    }

    fn visit_item_fn(&mut self, i: &'a syn::ItemFn) {
        if i.sig.unsafety.is_some() {
            push(i, RegionKind::FnImpl, self.regions);
        }
        syn::visit::visit_item_fn(self, i);
    }

    fn visit_impl_item_fn(&mut self, i: &'a syn::ImplItemFn) {
        if i.sig.unsafety.is_some() {
            push(i, RegionKind::FnImpl, self.regions);
        }
        syn::visit::visit_impl_item_fn(self, i);
    }

    fn visit_trait_item_fn(&mut self, i: &'a syn::TraitItemFn) {
        if i.sig.unsafety.is_some() {
            push(i, RegionKind::FnImpl, self.regions);
        }
        syn::visit::visit_trait_item_fn(self, i);
    }

    fn visit_item_trait(&mut self, i: &'a syn::ItemTrait) {
        if i.unsafety.is_some() {
            push(i, RegionKind::FnImpl, self.regions);
        }
        syn::visit::visit_item_trait(self, i);
    }

    fn visit_item_impl(&mut self, i: &'a syn::ItemImpl) {
        if i.unsafety.is_some() {
            push(i, RegionKind::FnImpl, self.regions);
        }
        syn::visit::visit_item_impl(self, i);
    }

    fn visit_item_foreign_mod(&mut self, i: &'a syn::ItemForeignMod) {
        if i.unsafety.is_some() {
            push(i, RegionKind::FnImpl, self.regions);
        }
        syn::visit::visit_item_foreign_mod(self, i);
    }

    fn visit_item_macro(&mut self, i: &'a ItemMacro) {
        // Macro bodies are opaque to syn; best-effort token scan for `unsafe`
        // keywords. macro_rules! definitions and invocations get identical
        // treatment on both branches (a template may expand N times; the
        // report carries that caveat).
        for tt in i.mac.tokens.clone().into_iter() {
            scan_tree(tt, self.offsets, &mut self.macro_internal.tokens, &mut self.macro_internal.lines);
        }
        // Deliberately do not recurse: there is no expanded AST to visit.
    }
}

fn scan_tree(tt: TokenTree, offsets: &[usize], count: &mut usize, lines: &mut BTreeSet<usize>) {
    match tt {
        TokenTree::Ident(id) => {
            if id == "unsafe" {
                let range = id.span().byte_range();
                let a = line_of(offsets, range.start);
                let b = line_of(offsets, range.end.saturating_sub(1).max(range.start));
                *count += 1;
                for l in a..=b {
                    lines.insert(l);
                }
            }
        }
        TokenTree::Group(g) => {
            for t in g.stream().into_iter() {
                scan_tree(t, offsets, count, lines);
            }
        }
        _ => {}
    }
}

/// Walk `<root>/kernel/src/**/*.rs`, excluding `<root>/kernel/src/tests/`.
pub fn collect_files(root: &Path) -> Vec<PathBuf> {
    let base = root.join("kernel").join("src");
    let mut out = Vec::new();
    let mut stack = vec![base.clone()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p == base.join("tests") {
                    continue; // excluded on both branches, per methodology
                }
                stack.push(p);
            } else if p.extension().map_or(false, |x| x == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Scan a whole source root. Returns per-file metrics plus the list of files
/// that failed to read or parse.
pub fn scan_root(root: &Path) -> (Vec<FileMetrics>, Vec<(PathBuf, String)>) {
    let files = collect_files(root);
    let mut metrics = Vec::new();
    let mut errors = Vec::new();
    for f in &files {
        let src = match fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                errors.push((f.clone(), e.to_string()));
                continue;
            }
        };
        let m = scan_str(f, &src);
        if !m.parse_ok {
            let msg = m.parse_error.clone().unwrap_or_default();
            errors.push((f.clone(), msg));
        }
        metrics.push(m);
    }
    (metrics, errors)
}
