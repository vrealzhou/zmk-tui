//! Small text-fitting helpers shared by the keymap and list renderers.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Truncate to at most `max` display columns without adding an ellipsis.
pub fn fit(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if width(text) <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > max {
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out
}

/// Truncate to `max` columns, ending with an ellipsis when shortened.
pub fn ellipsize(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return "…".into();
    }
    let mut out = fit(text, max - 1);
    out.push('…');
    out
}

/// Centre `text` in `max` columns. The result is never wider than `max`.
pub fn center(text: &str, max: usize) -> String {
    let fitted = fit(text, max);
    let text_width = width(&fitted);
    if text_width >= max {
        return fitted;
    }
    let left = (max - text_width) / 2;
    let mut out = String::with_capacity(max);
    for _ in 0..left {
        out.push(' ');
    }
    out.push_str(&fitted);
    while width(&out) < max {
        out.push(' ');
    }
    out
}

/// Truncate a string and pad it to exactly `max` columns.
pub fn fit_padded(text: &str, max: usize) -> String {
    let fitted = fit(text, max);
    let mut out = fitted;
    while width(&out) < max {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_respects_display_width() {
        assert_eq!(fit("SPACE", 3), "SPA");
        assert_eq!(fit("A", 3), "A");
        assert_eq!(fit("ＦＵＬＬ", 3), "Ｆ");
        assert_eq!(fit("abc", 0), "");
    }

    #[test]
    fn ellipsize_adds_ellipsis_when_truncated() {
        assert_eq!(ellipsize("Momentary Layer", 8), "Momenta…");
        assert_eq!(ellipsize("Base", 8), "Base");
    }

    #[test]
    fn center_pads_both_sides() {
        assert_eq!(center("A", 3), " A ");
        assert_eq!(center("SPACE", 3), "SPA");
        assert_eq!(width(&center("A", 4)), 4);
    }
}
