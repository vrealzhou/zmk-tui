//! Compact rendering of bindings for the keymap view and detail panel.

use crate::keycodes;
use crate::model::{Binding, Catalog, Keymap, Layer, ParamKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingLabel {
    /// Small header inside the key cap, e.g. `MT⇧`, `LT1`, `MO`.
    pub header: String,
    /// Primary label, e.g. `A`, `SPC`, `1`.
    pub main: String,
    /// Full devicetree-style description for the detail panel.
    pub detail: String,
}

impl BindingLabel {
    pub fn unknown(behavior_id: u32) -> Self {
        Self {
            header: "?".into(),
            main: format!("#{behavior_id}"),
            detail: format!("unknown behavior (id {behavior_id})"),
        }
    }
}

pub fn short_behavior_name(display_name: &str) -> String {
    let known = match display_name {
        "Key Press" => return String::new(),
        "Transparent" => return String::new(),
        "None" => return String::new(),
        "Momentary Layer" => "MO",
        "Toggle Layer" => "TG",
        "Layer-Tap" => "LT",
        "Mod-Tap" => "MT",
        "Sticky Key" => "SK",
        "Sticky Layer" => "SL",
        "Studio Unlock" => "UNL",
        "Bootloader" => "BOOT",
        "Reset" => "RST",
        "External Power" => "EXT",
        "Output Selection" => "OUT",
        "Grave/Escape" => "GESC",
        "Key Repeat" => "REP",
        "Key Toggle" => "TGL",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    let compact: String = display_name
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-' && *ch != '_')
        .collect();
    compact.chars().take(5).collect::<String>().to_uppercase()
}

fn layer_position(keymap: &Keymap, layer_id: u32) -> (String, String) {
    if let Some(index) = keymap.layer_index_by_id(layer_id) {
        let layer: &Layer = &keymap.layers[index];
        let name = layer.label(index);
        (index.to_string(), name)
    } else {
        (format!("#{layer_id}"), format!("unknown layer {layer_id}"))
    }
}

fn keycode_main(value: u32) -> String {
    keycodes::label(value)
}

/// QMK/Vial modifier masks use four left-hand bits plus one shared right-hand
/// flag, unlike ZMK's eight independent modifier bits.
fn qmk_modifier_symbols(mask: u32) -> String {
    let right = mask & 0x10 != 0;
    let mut symbols = String::new();
    for (bit, left_symbol, right_symbol) in [
        (0x01, "⌃", "R⌃"),
        (0x02, "⇧", "R⇧"),
        (0x04, "⌥", "R⌥"),
        (0x08, "⌘", "R⌘"),
    ] {
        if mask & bit != 0 {
            symbols.push_str(if right { right_symbol } else { left_symbol });
        }
    }
    symbols
}

fn qmk_modifier_names(mask: u32) -> String {
    let right = mask & 0x10 != 0;
    let mut names = Vec::new();
    for (bit, left_name, right_name) in [
        (0x01, "LCTL", "RCTL"),
        (0x02, "LSFT", "RSFT"),
        (0x04, "LALT", "RALT"),
        (0x08, "LGUI", "RGUI"),
    ] {
        if mask & bit != 0 {
            names.push(if right { right_name } else { left_name });
        }
    }
    if names.is_empty() {
        "none".into()
    } else {
        names.join("+")
    }
}

/// Render a single binding.
pub fn binding_label(catalog: &Catalog, keymap: &Keymap, binding: Binding) -> BindingLabel {
    if binding.is_empty() {
        return BindingLabel {
            header: String::new(),
            main: "—".into(),
            detail: "unassigned".into(),
        };
    }
    let Some(behavior) = catalog.get(binding.behavior_id) else {
        return BindingLabel::unknown(binding.behavior_id);
    };

    let name = behavior.display_name.as_str();
    let short = short_behavior_name(name);

    match name {
        "Key Press" => BindingLabel {
            header: String::new(),
            main: keycode_main(binding.param1),
            detail: format!("&kp {}", keycodes::name(binding.param1)),
        },
        "Transparent" => BindingLabel {
            header: String::new(),
            main: "TRNS".into(),
            detail: "&trans".into(),
        },
        "None" => BindingLabel {
            header: String::new(),
            main: "···".into(),
            detail: "&none".into(),
        },
        "Momentary Layer" => {
            let (position, layer_name) = layer_position(keymap, binding.param1);
            BindingLabel {
                header: "MO".into(),
                main: position.clone(),
                detail: format!("&mo {position} ({layer_name})"),
            }
        }
        "Toggle Layer" => {
            let (position, layer_name) = layer_position(keymap, binding.param1);
            BindingLabel {
                header: "TG".into(),
                main: position.clone(),
                detail: format!("&tog {position} ({layer_name})"),
            }
        }
        "Layer-Tap" => {
            let (position, layer_name) = layer_position(keymap, binding.param1);
            BindingLabel {
                header: format!("LT{position}"),
                main: keycode_main(binding.param2),
                detail: format!(
                    "&lt {position} {} ({layer_name})",
                    keycodes::name(binding.param2)
                ),
            }
        }
        "Mod-Tap" => {
            let modifier = match keycodes::modifier_symbol_for_keycode(binding.param1) {
                Some(symbol) => symbol.to_string(),
                None => keycodes::label(binding.param1),
            };
            BindingLabel {
                header: format!("MT{modifier}"),
                main: keycode_main(binding.param2),
                detail: format!(
                    "&mt {} {}",
                    keycodes::name(binding.param1),
                    keycodes::name(binding.param2)
                ),
            }
        }
        "Sticky Key" => BindingLabel {
            header: "SK".into(),
            main: keycode_main(binding.param1),
            detail: format!("&sk {}", keycodes::name(binding.param1)),
        },
        "Sticky Layer" => {
            let (position, layer_name) = layer_position(keymap, binding.param1);
            BindingLabel {
                header: "SL".into(),
                main: position.clone(),
                detail: format!("&sl {position} ({layer_name})"),
            }
        }
        "Studio Unlock" => BindingLabel {
            header: String::new(),
            main: "UNLOCK".into(),
            detail: "&studio_unlock".into(),
        },
        // Vial's pseudo behaviors carry QMK semantics: parameter 1 is a
        // 5-bit modifier mask, not a HID usage. Rendering them through the
        // generic path produced headers such as "MODTA" and values like "1 A".
        "Mod-Tap (Vial)" => {
            let mods = qmk_modifier_symbols(binding.param1);
            BindingLabel {
                header: format!("MT{mods}"),
                main: keycode_main(binding.param2),
                detail: format!(
                    "MT({}, {})",
                    qmk_modifier_names(binding.param1),
                    keycodes::name(binding.param2)
                ),
            }
        }
        "Modified Key" => BindingLabel {
            header: qmk_modifier_symbols(binding.param1),
            main: keycode_main(binding.param2),
            detail: format!(
                "{}({})",
                qmk_modifier_names(binding.param1),
                keycodes::name(binding.param2)
            ),
        },
        "Sticky Modifier" => BindingLabel {
            header: "SK".into(),
            main: qmk_modifier_symbols(binding.param1),
            detail: format!("OSM({})", qmk_modifier_names(binding.param1)),
        },
        "To Layer" => {
            let (position, layer_name) = layer_position(keymap, binding.param1);
            BindingLabel {
                header: "TO".into(),
                main: position.clone(),
                detail: format!("&to {position} ({layer_name})"),
            }
        }
        "Vial Keycode" => BindingLabel {
            header: "RAW".into(),
            main: format!("0x{:04X}", binding.param1 & 0xFFFF),
            detail: format!("QMK keycode 0x{:04X}", binding.param1 & 0xFFFF),
        },
        _ => generic_label(behavior, binding, &short),
    }
}

fn generic_label(behavior: &crate::model::Behavior, binding: Binding, short: &str) -> BindingLabel {
    let mut values = Vec::new();
    if let Some(set) = behavior.primary_param_set() {
        if let Some(param) = set.param1.first() {
            values.push(render_param(&param.kind, binding.param1));
        }
        if let Some(param) = set.param2.first() {
            values.push(render_param(&param.kind, binding.param2));
        }
    }
    if values.is_empty() && binding.param1 != 0 {
        values.push(binding.param1.to_string());
    }
    if values.is_empty() && binding.param2 != 0 {
        values.push(binding.param2.to_string());
    }

    let main = if values.is_empty() {
        behavior.display_name.clone()
    } else {
        values.join(" ")
    };
    let mut detail = format!(
        "&{}",
        behavior.display_name.replace(' ', "_").to_lowercase()
    );
    if binding.param1 != 0 || binding.param2 != 0 {
        detail.push_str(&format!(" {} {}", binding.param1, binding.param2));
    }

    BindingLabel {
        header: short.to_string(),
        main,
        detail,
    }
}

fn render_param(kind: &ParamKind, value: u32) -> String {
    match kind {
        ParamKind::Nil => String::new(),
        ParamKind::Constant(_) | ParamKind::Range { .. } => value.to_string(),
        ParamKind::HidUsage { .. } => keycodes::label(value),
        ParamKind::LayerId => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Behavior, ParamDesc, ParamSet};

    fn catalog() -> Catalog {
        let mut catalog = Catalog::default();
        catalog.insert(Behavior {
            id: 10,
            display_name: "Key Press".into(),
            metadata: vec![ParamSet {
                param1: vec![ParamDesc {
                    name: "Key".into(),
                    kind: ParamKind::HidUsage {
                        keyboard_max: 255,
                        consumer_max: 1023,
                    },
                }],
                param2: vec![],
            }],
        });
        catalog.insert(Behavior {
            id: 20,
            display_name: "Mod-Tap".into(),
            metadata: vec![ParamSet {
                param1: vec![ParamDesc {
                    name: "Modifier".into(),
                    kind: ParamKind::HidUsage {
                        keyboard_max: 255,
                        consumer_max: 1023,
                    },
                }],
                param2: vec![ParamDesc {
                    name: "Key".into(),
                    kind: ParamKind::HidUsage {
                        keyboard_max: 255,
                        consumer_max: 1023,
                    },
                }],
            }],
        });
        catalog.insert(Behavior {
            id: 30,
            display_name: "Momentary Layer".into(),
            metadata: vec![ParamSet {
                param1: vec![ParamDesc {
                    name: "Layer".into(),
                    kind: ParamKind::LayerId,
                }],
                param2: vec![],
            }],
        });
        catalog.insert(Behavior {
            id: 40,
            display_name: "Transparent".into(),
            metadata: vec![],
        });
        catalog.insert(Behavior {
            id: 50,
            display_name: "Custom Thing".into(),
            metadata: vec![ParamSet {
                param1: vec![ParamDesc {
                    name: "Constant".into(),
                    kind: ParamKind::Constant(0x11),
                }],
                param2: vec![ParamDesc {
                    name: "Layer".into(),
                    kind: ParamKind::LayerId,
                }],
            }],
        });
        catalog.insert(Behavior {
            id: 60,
            display_name: "Custom Plain".into(),
            metadata: vec![],
        });
        catalog
    }

    fn keymap() -> Keymap {
        Keymap {
            layers: vec![
                Layer {
                    id: 0,
                    name: "Base".into(),
                    bindings: vec![],
                },
                Layer {
                    id: 7,
                    name: "Lower".into(),
                    bindings: vec![],
                },
            ],
            available_layers: 2,
            max_layer_name_length: 16,
        }
    }

    #[test]
    fn key_press_uses_keycode_label() {
        let label = binding_label(
            &catalog(),
            &keymap(),
            Binding {
                behavior_id: 10,
                param1: 0x0007002C,
                param2: 0,
            },
        );
        assert_eq!(label.main, "SPC");
        assert_eq!(label.header, "");
        assert_eq!(label.detail, "&kp SPACE");
    }

    #[test]
    fn mod_tap_header_includes_modifier_symbol() {
        let label = binding_label(
            &catalog(),
            &keymap(),
            Binding {
                behavior_id: 20,
                param1: 0x000700E1, // LEFT_SHIFT
                param2: 0x00070004, // A
            },
        );
        assert_eq!(label.header, "MT⇧");
        assert_eq!(label.main, "A");
        assert_eq!(label.detail, "&mt LEFT_SHIFT A");
    }

    #[test]
    fn layer_ids_resolve_to_positions() {
        let label = binding_label(
            &catalog(),
            &keymap(),
            Binding {
                behavior_id: 30,
                param1: 7,
                param2: 0,
            },
        );
        assert_eq!(label.header, "MO");
        assert_eq!(label.main, "1");
        assert!(label.detail.contains("Lower"));
    }

    #[test]
    fn transparent_and_unassigned_are_distinct() {
        let transparent = binding_label(
            &catalog(),
            &keymap(),
            Binding {
                behavior_id: 40,
                param1: 0,
                param2: 0,
            },
        );
        assert_eq!(transparent.main, "TRNS");

        let unassigned = binding_label(&catalog(), &keymap(), Binding::default());
        assert_eq!(unassigned.main, "—");
        assert_eq!(unassigned.detail, "unassigned");
    }

    #[test]
    fn short_names_are_bounded() {
        assert_eq!(short_behavior_name("Momentary Layer"), "MO");
        assert_eq!(short_behavior_name("RGB Underglow Toggle"), "RGBUN");
        assert_eq!(short_behavior_name("Key Press"), "");
    }

    #[test]
    fn generic_behaviors_render_all_param_kinds() {
        let label = binding_label(
            &catalog(),
            &keymap(),
            Binding {
                behavior_id: 50,
                param1: 0x11,
                param2: 7,
            },
        );
        assert_eq!(label.header, "CUSTO");
        assert_eq!(label.main, "17 7");
        assert_eq!(label.detail, "&custom_thing 17 7");

        let plain = binding_label(
            &catalog(),
            &keymap(),
            Binding {
                behavior_id: 60,
                param1: 0,
                param2: 0,
            },
        );
        assert_eq!(plain.main, "Custom Plain");
        assert_eq!(plain.detail, "&custom_plain");
    }

    #[test]
    fn unknown_behavior_and_layer_are_visible() {
        let unknown = binding_label(
            &catalog(),
            &keymap(),
            Binding {
                behavior_id: 999,
                param1: 0,
                param2: 0,
            },
        );
        assert_eq!(unknown.header, "?");
        assert!(unknown.detail.contains("999"));
    }

    #[test]
    fn vial_pseudo_behaviors_use_qmk_modifier_semantics() {
        let catalog = crate::vial::keycode::catalog();
        let keymap = Keymap::default();

        let mod_tap = binding_label(
            &catalog,
            &keymap,
            Binding {
                behavior_id: crate::vial::keycode::behavior::MT,
                param1: 0x02,
                param2: 0x0007_0004,
            },
        );
        assert_eq!(mod_tap.header, "MT⇧");
        assert_eq!(mod_tap.main, "A");
        assert!(mod_tap.detail.contains("LSFT"), "{}", mod_tap.detail);

        let modified = binding_label(
            &catalog,
            &keymap,
            Binding {
                behavior_id: crate::vial::keycode::behavior::KP_MODS,
                param1: 0x03,
                param2: 0x0007_0005,
            },
        );
        assert_eq!(modified.header, "⌃⇧");
        assert_eq!(modified.main, "B");
        assert!(modified.detail.contains("LCTL+LSFT"), "{}", modified.detail);

        let raw = binding_label(
            &catalog,
            &keymap,
            Binding {
                behavior_id: crate::vial::keycode::behavior::RAW,
                param1: 0x7777,
                param2: 0,
            },
        );
        assert_eq!(raw.header, "RAW");
        assert_eq!(raw.main, "0x7777");
    }
}
