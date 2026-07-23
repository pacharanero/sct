// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared human-readable formatting helpers: byte sizes, thousands-separated
//! counts, and English pluralisation of a trailing noun. Several commands
//! (`info`, `trud`, `diff`, `size`) previously carried their own copies of
//! this logic; this module is the single tested source.

/// Format a byte count as a human-readable size (`"512 B"`, `"1.0 KB"`,
/// `"5.0 MB"`, `"2.0 GB"`).
pub fn human_bytes(bytes: u64) -> String {
    const GB: u64 = 1 << 30;
    const MB: u64 = 1 << 20;
    const KB: u64 = 1 << 10;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Format an integer with thousands separators (`1234567` -> `"1,234,567"`).
pub fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

/// Render a count with its noun, pluralised with a trailing `s` unless `n == 1`
/// (`(1, "concept")` -> `"1 concept"`, `(5, "concept")` -> `"5 concepts"`).
pub fn plural_count(n: u64, singular: &str) -> String {
    if n == 1 {
        format!("1 {singular}")
    } else {
        format!("{} {singular}s", fmt_count(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_sub_kilobyte() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1023), "1023 B");
    }

    #[test]
    fn human_bytes_kilobytes() {
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(2048), "2.0 KB");
    }

    #[test]
    fn human_bytes_megabytes() {
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn human_bytes_gigabytes() {
        assert_eq!(human_bytes(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    #[test]
    fn fmt_count_thousands() {
        assert_eq!(fmt_count(1_234_567), "1,234,567");
        assert_eq!(fmt_count(831_132), "831,132");
        assert_eq!(fmt_count(42), "42");
        assert_eq!(fmt_count(0), "0");
    }

    #[test]
    fn plural_count_singular() {
        assert_eq!(plural_count(1, "concept"), "1 concept");
    }

    #[test]
    fn plural_count_plural() {
        assert_eq!(plural_count(0, "concept"), "0 concepts");
        assert_eq!(plural_count(2, "concept"), "2 concepts");
        assert_eq!(plural_count(1_234, "concept"), "1,234 concepts");
    }
}
