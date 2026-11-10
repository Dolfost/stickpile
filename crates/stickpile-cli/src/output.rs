//! Human-readable output: aligned tables and value formatting.

use std::fmt::Write as _;

use serde::Serialize;
use unicode_width::UnicodeWidthStr;

/// Column-aligned table; the last column is never padded.
#[derive(Debug, Default)]
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

/// Longest cell content before truncation with `…`.
const MAX_CELL: usize = 48;

impl Table {
    pub fn new(headers: &[&str]) -> Table {
        Table {
            headers: headers.iter().map(|h| h.to_string()).collect(),
            rows: Vec::new(),
        }
    }

    pub fn row(&mut self, cells: Vec<String>) {
        self.rows.push(
            cells
                .into_iter()
                .map(|c| truncate(&c.replace(['\n', '\t'], " "), MAX_CELL))
                .collect(),
        );
    }

    pub fn render(&self) -> String {
        let cols = self.headers.len();
        let mut widths: Vec<usize> = self.headers.iter().map(|h| h.width()).collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate().take(cols) {
                widths[i] = widths[i].max(cell.width());
            }
        }
        let mut out = String::new();
        let mut line = |cells: &[String]| {
            let mut s = String::new();
            for (i, cell) in cells.iter().enumerate().take(cols) {
                if i + 1 == cols {
                    s.push_str(cell);
                } else {
                    s.push_str(cell);
                    s.push_str(&" ".repeat(widths[i] - cell.width() + 2));
                }
            }
            let _ = writeln!(out, "{}", s.trim_end());
        };
        line(&self.headers);
        for row in &self.rows {
            line(row);
        }
        out
    }

    pub fn print(&self) {
        print!("{}", self.render());
    }
}

/// Cut to at most `max` display columns, ending in `…` when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// `2026-11-10 14:03` (UTC) for unix seconds.
pub fn time(ts: i64) -> String {
    let fmt = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]");
    time::OffsetDateTime::from_unix_timestamp(ts)
        .ok()
        .and_then(|t| t.format(&fmt).ok())
        .unwrap_or_else(|| ts.to_string())
}

pub fn opt_time(ts: Option<i64>) -> String {
    ts.map(time).unwrap_or_else(|| "-".into())
}

pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub fn opt<T: ToString>(v: Option<T>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "-".into())
}

/// snake_case name of a serde enum value (`Visibility::Public` -> `public`).
pub fn name<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

pub fn json<T: Serialize + ?Sized>(v: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_aligns_wide_characters() {
        let mut t = Table::new(&["NAME", "N"]);
        t.row(vec!["猫猫".into(), "1".into()]);
        t.row(vec!["dog".into(), "22".into()]);
        let out = t.render();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "NAME  N");
        assert_eq!(lines[1], "猫猫  1");
        assert_eq!(lines[2], "dog   22");
    }

    #[test]
    fn truncates_long_cells() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
        assert_eq!(truncate("猫猫猫", 4), "猫…");
    }

    #[test]
    fn formats_values() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(10 * 1024 * 1024), "10.0 MiB");
        assert_eq!(time(0), "1970-01-01 00:00");
        assert_eq!(opt_time(None), "-");
        assert_eq!(name(&stickpile_api::Visibility::Unlisted), "unlisted");
    }
}
