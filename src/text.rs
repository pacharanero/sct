// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Output-context encoding. Stored and structured values remain unchanged.

pub(crate) fn single_line(value: &str) -> std::borrow::Cow<'_, str> {
    let unsafe_char = |c: char| c.is_control() || (c.is_whitespace() && c != ' ');
    if value.chars().any(unsafe_char) {
        std::borrow::Cow::Owned(
            value
                .chars()
                .map(|c| if unsafe_char(c) { ' ' } else { c })
                .collect(),
        )
    } else {
        std::borrow::Cow::Borrowed(value)
    }
}

/// Plain display text for Markdown headings, prose and table cells, not code spans.
#[cfg(feature = "cli")]
pub(crate) fn markdown_text(value: &str) -> String {
    let mut out = String::new();
    for (i, word) in single_line(value).split_whitespace().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        for ch in word.chars() {
            match ch {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                ch if ch.is_ascii_punctuation() => {
                    out.push('\\');
                    out.push(ch);
                }
                ch => out.push(ch),
            }
        }
    }
    out
}
