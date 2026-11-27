use std::path::Path;
use std::process::exit;

use unsafe_meter::{scan_root, FileMetrics};

fn totals(metrics: &[FileMetrics]) -> Totals {
    let mut t = Totals::default();
    for m in metrics {
        t.files += 1;
        t.total_lines += m.total_lines;
        t.block_count += m.block_count;
        t.block_lines += m.block_lines.len();
        t.fnimpl_count += m.fnimpl_count;
        t.fnimpl_lines += m.fnimpl_lines.len();
        t.macro_tokens += m.macro_internal.tokens;
        t.macro_line_union += m.macro_internal.lines.len();
    }
    t.unsafe_lines = t.block_lines + t.fnimpl_lines;
    t
}

#[derive(Default)]
struct Totals {
    files: usize,
    total_lines: usize,
    block_count: usize,
    block_lines: usize,
    fnimpl_count: usize,
    fnimpl_lines: usize,
    unsafe_lines: usize,
    macro_tokens: usize,
    macro_line_union: usize,
}

fn pct(a: usize, b: usize) -> String {
    if b == 0 {
        return "n/a".into();
    }
    format!("{:.2}%", (a as f64 * 100.0 / b as f64))
}

fn human(m: &[FileMetrics]) -> String {
    let t = totals(m);
    let mut out = String::new();
    out.push_str(&format!(
        "files scanned: {} | physical lines (denominator): {}\n",
        t.files, t.total_lines
    ));
    out.push_str(&format!(
        "block-series:   {:>6} blocks | {:>6} lines | {}\n",
        t.block_count,
        t.block_lines,
        pct(t.block_lines, t.total_lines)
    ));
    out.push_str(&format!(
        "fn/impl-series: {:>6} items  | {:>6} lines | {}\n",
        t.fnimpl_count,
        t.fnimpl_lines,
        pct(t.fnimpl_lines, t.total_lines)
    ));
    out.push_str(&format!(
        "TOTAL unsafe:   {:>6}         | {:>6} lines | {}\n",
        t.unsafe_lines,
        t.unsafe_lines,
        pct(t.unsafe_lines, t.total_lines)
    ));
    out.push_str(&format!(
        "macro-internal (approximate): {} unsafe tokens | {} lines (may overlap the two series)\n",
        t.macro_tokens, t.macro_line_union
    ));
    out
}

fn top_table(m: &[FileMetrics], top: usize) -> String {
    let mut sorted: Vec<&FileMetrics> = m.iter().collect();
    sorted.sort_by_key(|f| std::cmp::Reverse(f.unsafe_lines()));
    let mut out = String::new();
    out.push_str(&format!(
        "{:<6} {:<6} {:<6} {:<6} {:<8} {}\n",
        "UNSAFE", "BLK", "FNIMPL", "LINES", "TOT", "FILE"
    ));
    let shown = sorted.len().min(top);
    for f in sorted.iter().take(shown) {
        out.push_str(&format!(
            "{:<6} {:<6} {:<6} {:<6} {:<8} {}\n",
            f.unsafe_lines(),
            f.block_count,
            f.fnimpl_count,
            f.block_lines.len() + f.fnimpl_lines.len(),
            f.total_lines,
            strip_root(&f.path),
        ));
    }
    out
}

fn strip_root(p: &Path) -> String {
    // keep kernel/src/... relative suffix for stable display
    let s = p.to_string_lossy();
    match s.find("kernel/src") {
        Some(i) => s[i..].to_string(),
        None => s.to_string(),
    }
}

fn json_out(m: &[FileMetrics]) -> String {
    let t = totals(m);
    let mut files = Vec::new();
    let mut sorted: Vec<&FileMetrics> = m.iter().collect();
    sorted.sort_by_key(|f| std::cmp::Reverse(f.unsafe_lines()));
    for f in sorted {
        files.push(format!(
            "{{\"file\":\"{}\",\"total_lines\":{},\"block_count\":{},\"block_lines\":{},\"fnimpl_count\":{},\"fnimpl_lines\":{},\"unsafe_lines\":{},\"macro_tokens\":{},\"macro_lines\":{},\"parse_ok\":{}}}",
            escape(&strip_root(&f.path)),
            f.total_lines,
            f.block_count,
            f.block_lines.len(),
            f.fnimpl_count,
            f.fnimpl_lines.len(),
            f.unsafe_lines(),
            f.macro_internal.tokens,
            f.macro_internal.lines.len(),
            f.parse_ok
        ));
    }
    format!(
        "{{\"files\":{},\"total_lines\":{},\"block_count\":{},\"block_lines\":{},\"fnimpl_count\":{},\"fnimpl_lines\":{},\"unsafe_lines\":{},\"macro_tokens\":{},\"macro_lines_union\":{},\"per_file\":[{}]}}\n",
        t.files,
        t.total_lines,
        t.block_count,
        t.block_lines,
        t.fnimpl_count,
        t.fnimpl_lines,
        t.unsafe_lines,
        t.macro_tokens,
        t.macro_line_union,
        files.join(",")
    )
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("lines") => {
            // Debug aid for hand-verification: per-line series attribution of
            // one file. usage: unsafe-meter lines <src-root> <path-under-root>
            let root = args.get(1).expect("usage: unsafe-meter lines <src-root> <file>");
            let rel = args.get(2).expect("usage: unsafe-meter lines <src-root> <file>");
            let f = Path::new(root).join(rel);
            let src = std::fs::read_to_string(&f).unwrap_or_else(|e| {
                eprintln!("read {}: {}", f.display(), e);
                exit(1);
            });
            let m = unsafe_meter::scan_str(&f, &src);
            if !m.parse_ok {
                eprintln!("parse error: {:?}", m.parse_error);
                exit(2);
            }
            for line in 1..=m.total_lines {
                let tag = if m.fnimpl_lines.contains(&line) {
                    "FN/IMPL"
                } else if m.block_lines.contains(&line) {
                    "BLOCK"
                } else {
                    "-"
                };
                println!("{:>6} {:>8} {}", line, tag, src.lines().nth(line - 1).unwrap_or(""));
            }
        }
        Some("scan") => {
            let root = args.get(1).expect("usage: unsafe-meter scan <src-root> [--json] [--top N]");
            let json = args.iter().any(|a| a == "--json");
            let top = args
                .iter()
                .position(|a| a == "--top")
                .and_then(|i| args.get(i + 1))
                .and_then(|v| v.parse().ok())
                .unwrap_or(20usize);
            let (metrics, errors) = scan_root(Path::new(root));
            if json {
                print!("{}", json_out(&metrics));
            } else {
                println!("{}", human(&metrics));
                print!("\n{}", top_table(&metrics, top));
            }
            if !errors.is_empty() {
                eprintln!("\nPARSE/READ ERRORS ({}):", errors.len());
                for (p, e) in &errors {
                    eprintln!("  {}: {}", p.display(), e);
                }
                exit(2);
            }
        }
        _ => {
            eprintln!(
                "usage:\n  unsafe-meter scan <src-root> [--json] [--top N]\n\nWalks <src-root>/kernel/src/**/*.rs (kernel/src/tests excluded)."
            );
            exit(1);
        }
    }
}
