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
    // Combining sequences (for example an emoji followed by U+FE0F) measure
    // wider than the sum of their individual characters; trim until the
    // contract "never wider than max" holds.
    while width(&out) > max {
        out.pop();
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
    fn fit_never_exceeds_the_column_budget_for_combining_sequences() {
        // U+2600 followed by the emoji variation selector measures two columns
        // as a sequence but only one column per code point.
        let sun = "\u{2600}\u{FE0F}";
        let fitted = fit(sun, 1);
        assert!(width(&fitted) <= 1, "fitted width {}", width(&fitted));
        let ellipsized = ellipsize(&format!("{sun}x"), 2);
        assert!(
            width(&ellipsized) <= 2,
            "ellipsized width {}",
            width(&ellipsized)
        );
        assert!(width(&center(sun, 1)) <= 1);
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
