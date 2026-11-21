//! HID keycode metadata used for labels, search and value encoding.
//!
//! The table in [`data`] is generated from ZMK's `keys.h` and `hid_usage.h`
//! (MIT licensed); see `tools/gen_keycodes.py`.

mod data;

use std::collections::HashMap;
use std::sync::OnceLock;

pub use data::KEYCODES;

pub const KEYBOARD_PAGE: u8 = 0x07;
pub const CONSUMER_PAGE: u8 = 0x0C;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Letters,
    Numbers,
    Punctuation,
    Editing,
    Navigation,
    Modifiers,
    Function,
    Keypad,
    Media,
    System,
    Other,
}

impl Category {
    pub const ALL: [Category; 11] = [
        Category::Letters,
        Category::Numbers,
        Category::Punctuation,
        Category::Editing,
        Category::Navigation,
        Category::Modifiers,
        Category::Function,
        Category::Keypad,
        Category::Media,
        Category::System,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Letters => "Letters",
            Category::Numbers => "Numbers",
            Category::Punctuation => "Symbols",
            Category::Editing => "Editing",
            Category::Navigation => "Navigation",
            Category::Modifiers => "Modifiers",
            Category::Function => "Function",
            Category::Keypad => "Keypad",
            Category::Media => "Media",
            Category::System => "System",
            Category::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Keycode {
    /// Encoded ZMK keycode: modifier byte, page byte, usage ID.
    pub value: u32,
    pub page: u8,
    pub id: u16,
    /// Implicit modifier mask (from `LS(...)`-style keycodes).
    pub mods: u8,
    /// Canonical ZMK keycode name.
    pub name: &'static str,
    /// Other ZMK names that resolve to this keycode.
    pub aliases: &'static [&'static str],
    /// Compact terminal label.
    pub label: &'static str,
    pub category: Category,
}

impl Keycode {
    pub fn modifier_symbols(&self) -> String {
        modifier_symbols(self.mods)
    }

    /// Full name including implicit modifiers, e.g. `LSHIFT + A`.
    pub fn describe(&self) -> String {
        if self.mods == 0 {
            self.name.to_string()
        } else {
            format!("{} + {}", modifier_symbols_ascii(self.mods), self.name)
        }
    }
}

impl PartialEq for Keycode {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl Eq for Keycode {}

pub fn all() -> &'static [Keycode] {
    KEYCODES
}

fn by_value_map() -> &'static HashMap<u32, &'static Keycode> {
    static MAP: OnceLock<HashMap<u32, &'static Keycode>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map = HashMap::with_capacity(KEYCODES.len());
        for keycode in KEYCODES {
            map.entry(keycode.value).or_insert(keycode);
        }
        map
    })
}

fn by_page_id_map() -> &'static HashMap<(u8, u16), &'static Keycode> {
    static MAP: OnceLock<HashMap<(u8, u16), &'static Keycode>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map = HashMap::with_capacity(KEYCODES.len());
        for keycode in KEYCODES {
            if keycode.mods == 0 {
                map.entry((keycode.page, keycode.id)).or_insert(keycode);
            }
        }
        map
    })
}

pub fn by_value(value: u32) -> Option<&'static Keycode> {
    by_value_map().get(&value).copied()
}

pub fn by_page_id(page: u8, id: u16) -> Option<&'static Keycode> {
    by_page_id_map().get(&(page, id)).copied()
}

/// Resolve a ZMK keycode name (or alias), case-insensitively.
pub fn from_name(name: &str) -> Option<&'static Keycode> {
    let wanted = name.trim();
    KEYCODES
        .iter()
        .find(|keycode| keycode.name.eq_ignore_ascii_case(wanted))
        .or_else(|| {
            KEYCODES.iter().find(|keycode| {
                keycode
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(wanted))
            })
        })
}

pub fn modifier_symbols(mods: u8) -> String {
    let mut symbols = String::new();
    for (bit, symbol) in [
        (0x01, "⌃"),
        (0x02, "⇧"),
        (0x04, "⌥"),
        (0x08, "⌘"),
        (0x10, "R⌃"),
        (0x20, "R⇧"),
        (0x40, "R⌥"),
        (0x80, "R⌘"),
    ] {
        if mods & bit != 0 {
            symbols.push_str(symbol);
        }
    }
    symbols
}

pub fn modifier_symbols_ascii(mods: u8) -> String {
    let mut parts = Vec::new();
    for (bit, name) in [
        (0x01, "LCTRL"),
        (0x02, "LSHIFT"),
        (0x04, "LALT"),
        (0x08, "LGUI"),
        (0x10, "RCTRL"),
        (0x20, "RSHIFT"),
        (0x40, "RALT"),
        (0x80, "RGUI"),
    ] {
        if mods & bit != 0 {
            parts.push(name);
        }
    }
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join("+")
    }
}

/// Compact terminal label for a raw `&kp` parameter.
pub fn label(value: u32) -> String {
    if value == 0 {
        return "—".into();
    }
    if let Some(keycode) = by_value(value) {
        return keycode.label.to_string();
    }
    let page = ((value >> 16) & 0xFF) as u8;
    let usage = (value & 0xFFFF) as u16;
    let mods = ((value >> 24) & 0xFF) as u8;
    if let Some(base) = by_page_id(page, usage) {
        if mods == 0 {
            return base.label.to_string();
        }
        return format!("{}{}", modifier_symbols(mods), base.label);
    }
    format!("0x{value:08X}")
}

/// Full ZMK keycode name for a value, including unusual modifier combinations.
pub fn name(value: u32) -> String {
    if value == 0 {
        return "none".into();
    }
    if let Some(keycode) = by_value(value) {
        return keycode.name.to_string();
    }
    let page = ((value >> 16) & 0xFF) as u8;
    let usage = (value & 0xFFFF) as u16;
    let mods = ((value >> 24) & 0xFF) as u8;
    if let Some(base) = by_page_id(page, usage) {
        if mods == 0 {
            return base.name.to_string();
        }
        return format!("{}+{}", modifier_symbols_ascii(mods), base.name);
    }
    format!("0x{value:08X}")
}

pub fn describe(value: u32) -> String {
    let name = name(value);
    if value == 0 {
        name
    } else {
        format!("{name} (0x{value:08X})")
    }
}

/// True when the value is a modifier key (`&kp LSHIFT` and friends).
pub fn modifier_symbol_for_keycode(value: u32) -> Option<&'static str> {
    let keycode = by_value(value)?;
    if keycode.mods != 0 || keycode.page != KEYBOARD_PAGE {
        return None;
    }
    match keycode.id {
        224 => Some("⌃"),
        225 => Some("⇧"),
        226 => Some("⌥"),
        227 => Some("⌘"),
        228 => Some("R⌃"),
        229 => Some("R⇧"),
        230 => Some("R⌥"),
        231 => Some("R⌘"),
        _ => None,
    }
}

/// Filter keycodes by name, alias or label. Empty queries return everything.
pub fn search(query: &str) -> Vec<&'static Keycode> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return KEYCODES.iter().collect();
    }
    let terms: Vec<&str> = query.split_whitespace().collect();

    let mut scored: Vec<(i32, usize, &'static Keycode)> = KEYCODES
        .iter()
        .enumerate()
        .filter_map(|(index, keycode)| {
            let mut total = 0;
            for term in &terms {
                let score = term_score(term, keycode)?;
                total += score;
            }
            Some((total, index, keycode))
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, keycode)| keycode).collect()
}

fn term_score(term: &str, keycode: &Keycode) -> Option<i32> {
    let mut best = None;
    for candidate in std::iter::once(keycode.name)
        .chain(keycode.aliases.iter().copied())
        .chain(std::iter::once(keycode.label))
    {
        let candidate = candidate.to_lowercase();
        let score = if candidate == term {
            1000
        } else if candidate.starts_with(term) {
            500
        } else if candidate.contains(term) {
            250
        } else if is_subsequence(term, &candidate) {
            100
        } else {
            continue;
        };
        best = Some(best.map_or(score, |current: i32| current.max(score)));
    }
    best
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    needle.chars().all(|wanted| chars.any(|ch| ch == wanted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_known_keycodes() {
        assert_eq!(by_value(0x00070004).unwrap().name, "A");
        assert_eq!(label(0x00070028), "ENT");
        assert_eq!(label(0x0007002C), "SPC");
        assert_eq!(label(0x0207001E), "!");
        assert_eq!(name(0x0207001E), "EXCLAMATION");
        assert_eq!(from_name("enter").unwrap().name, "RETURN");
        assert_eq!(from_name("N1").unwrap().name, "NUMBER_1");
        assert_eq!(from_name("C_VOL_UP").unwrap().name, "C_VOLUME_UP");
        assert_eq!(label(0x000C00E9), "VOL+");
    }

    #[test]
    fn labels_unknown_values_without_panicking() {
        assert_eq!(label(0), "—");
        assert_eq!(label(0x000700FF), "0x000700FF");
        assert_eq!(name(0x000700FF), "0x000700FF");
    }

    #[test]
    fn modifiers_on_known_usages_are_prefixed() {
        // LSHIFT + A, a combination not present as its own keycode.
        assert_eq!(label(0x02070004), "⇧A");
        assert_eq!(name(0x02070004), "LSHIFT+A");
        assert_eq!(label(0x03070004), "⌃⇧A");
    }

    #[test]
    fn search_prefers_prefix_matches() {
        let results = search("vol");
        assert!(!results.is_empty());
        assert!(results[0].name.contains("VOL"));
        assert!(
            search("enter")
                .iter()
                .any(|keycode| keycode.name == "RETURN")
        );
        assert!(search("!").iter().any(|keycode| keycode.label == "!"));
        assert_eq!(search("   ").len(), all().len());
    }

    #[test]
    fn category_labels_are_stable() {
        let expected = [
            (Category::Letters, "Letters"),
            (Category::Numbers, "Numbers"),
            (Category::Punctuation, "Symbols"),
            (Category::Editing, "Editing"),
            (Category::Navigation, "Navigation"),
            (Category::Modifiers, "Modifiers"),
            (Category::Function, "Function"),
            (Category::Keypad, "Keypad"),
            (Category::Media, "Media"),
            (Category::System, "System"),
            (Category::Other, "Other"),
        ];
        for (category, label) in expected {
            assert_eq!(category.label(), label);
        }
    }

    #[test]
    fn keycode_describe_symbols_and_equality() {
        let a = by_value(0x0007_0004).expect("A");
        let q = by_value(0x0007_0014).expect("Q");
        assert_eq!(a.describe(), "A");
        assert_eq!(a.modifier_symbols(), "");
        assert_eq!(a, by_value(0x0007_0004).unwrap());
        assert_ne!(a, q);

        let shifted = by_value(0x0207_001E).expect("shifted exclamation");
        assert_eq!(shifted.modifier_symbols(), "⇧");
        assert_eq!(shifted.describe(), "LSHIFT + EXCLAMATION");
    }

    #[test]
    fn free_describe_handles_zero_known_and_unknown_values() {
        assert_eq!(describe(0), name(0));
        assert_eq!(describe(0x0007_0004), "A (0x00070004)");
        assert_eq!(describe(0x0007_00FF), "0x000700FF (0x000700FF)");
    }

    #[test]
    fn modifier_symbols_cover_all_eight_modifier_keys() {
        let expected = [
            (0x0007_00E0, "⌃"),
            (0x0007_00E1, "⇧"),
            (0x0007_00E2, "⌥"),
            (0x0007_00E3, "⌘"),
            (0x0007_00E4, "R⌃"),
            (0x0007_00E5, "R⇧"),
            (0x0007_00E6, "R⌥"),
            (0x0007_00E7, "R⌘"),
        ];
        for (value, symbol) in expected {
            assert_eq!(modifier_symbol_for_keycode(value), Some(symbol));
        }
        assert_eq!(modifier_symbol_for_keycode(0x0007_0004), None);
        assert_eq!(modifier_symbol_for_keycode(0xFFFF_FFFF), None);
    }

    #[test]
    fn every_keycode_is_unique_by_value() {
        let mut values: Vec<u32> = all().iter().map(|keycode| keycode.value).collect();
        values.sort_unstable();
        let before = values.len();
        values.dedup();
        assert_eq!(before, values.len(), "duplicate keycode values");
    }
}
