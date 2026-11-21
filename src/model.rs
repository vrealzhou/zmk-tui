//! Transport-independent domain types for a ZMK Studio device.

use std::collections::BTreeMap;

/// Metadata returned by `core.get_device_info`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    pub name: String,
    pub serial_number: Vec<u8>,
}

/// Metadata returned by `core.get_lock_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    Locked,
    Unlocked,
}

impl LockState {
    pub fn is_locked(self) -> bool {
        matches!(self, Self::Locked)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::Unlocked => "unlocked",
        }
    }
}

/// One binding at a key position.
///
/// `behavior_id` is a device-local ID (a CRC of the behavior's devicetree
/// node name), not a stable global constant. Use [`Catalog`] to resolve it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Binding {
    pub behavior_id: u32,
    pub param1: u32,
    pub param2: u32,
}

impl Binding {
    pub fn is_empty(&self) -> bool {
        self.behavior_id == 0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layer {
    pub id: u32,
    pub name: String,
    pub bindings: Vec<Binding>,
}

impl Layer {
    pub fn label(&self, index: usize) -> String {
        if self.name.is_empty() {
            format!("Layer {index}")
        } else {
            self.name.clone()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keymap {
    pub layers: Vec<Layer>,
    pub available_layers: u32,
    pub max_layer_name_length: usize,
}

impl Keymap {
    pub fn layer_index_by_id(&self, layer_id: u32) -> Option<usize> {
        self.layers.iter().position(|layer| layer.id == layer_id)
    }

    pub fn layer_label_by_id(&self, layer_id: u32) -> Option<String> {
        self.layer_index_by_id(layer_id)
            .map(|index| self.layers[index].label(index))
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }
}

/// Geometry from `keymap.get_physical_layouts`, in key units.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct KeyGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub rotation_x: f32,
    pub rotation_y: f32,
}

impl KeyGeometry {
    pub fn from_centi_units(
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        rotation: i32,
        rotation_x: i32,
        rotation_y: i32,
    ) -> Self {
        Self {
            x: x as f32 / 100.0,
            y: y as f32 / 100.0,
            width: width as f32 / 100.0,
            height: height as f32 / 100.0,
            rotation: rotation as f32 / 100.0,
            rotation_x: rotation_x as f32 / 100.0,
            rotation_y: rotation_y as f32 / 100.0,
        }
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhysicalLayout {
    pub name: String,
    pub keys: Vec<KeyGeometry>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhysicalLayouts {
    pub active_index: usize,
    pub layouts: Vec<PhysicalLayout>,
}

impl PhysicalLayouts {
    pub fn active(&self) -> Option<&PhysicalLayout> {
        self.layouts.get(self.active_index)
    }

    pub fn active_or_first(&self) -> Option<&PhysicalLayout> {
        self.layouts
            .get(self.active_index)
            .or_else(|| self.layouts.first())
    }
}

/// One selectable parameter value advertised by a behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDesc {
    pub name: String,
    pub kind: ParamKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    Nil,
    Constant(u32),
    Range {
        min: i32,
        max: i32,
    },
    HidUsage {
        keyboard_max: u32,
        consumer_max: u32,
    },
    LayerId,
}

/// A set of parameter descriptions. Most behaviors have one set; a behavior
/// with alternative signatures (for example a custom behavior) can have more.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParamSet {
    pub param1: Vec<ParamDesc>,
    pub param2: Vec<ParamDesc>,
}

impl ParamSet {
    pub fn has_params(&self) -> bool {
        !self.param1.is_empty() || !self.param2.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Behavior {
    pub id: u32,
    pub display_name: String,
    pub metadata: Vec<ParamSet>,
}

impl Behavior {
    pub fn has_params(&self) -> bool {
        self.metadata.iter().any(ParamSet::has_params)
    }

    /// The first metadata set that describes any parameters, falling back to
    /// the first set (or `None` when the device advertises no metadata).
    pub fn primary_param_set(&self) -> Option<&ParamSet> {
        self.metadata
            .iter()
            .find(|set| set.has_params())
            .or_else(|| self.metadata.first())
    }
}

/// Behaviors advertised by the device, keyed by local ID.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    behaviors: BTreeMap<u32, Behavior>,
}

impl Catalog {
    pub fn insert(&mut self, behavior: Behavior) {
        self.behaviors.insert(behavior.id, behavior);
    }

    pub fn get(&self, id: u32) -> Option<&Behavior> {
        self.behaviors.get(&id)
    }

    pub fn name(&self, id: u32) -> Option<&str> {
        self.get(id).map(|behavior| behavior.display_name.as_str())
    }

    pub fn len(&self) -> usize {
        self.behaviors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.behaviors.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Behavior> {
        self.behaviors.values()
    }

    /// Behaviors sorted by display name, for pickers.
    pub fn sorted(&self) -> Vec<&Behavior> {
        let mut all: Vec<&Behavior> = self.behaviors.values().collect();
        all.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        all
    }

    pub fn find_by_name(&self, name: &str) -> Option<&Behavior> {
        let wanted = name.trim();
        self.behaviors
            .values()
            .find(|behavior| behavior.display_name.eq_ignore_ascii_case(wanted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_converts_centi_units() {
        let key = KeyGeometry::from_centi_units(150, 250, 100, 100, 0, 0, 0);
        assert_eq!(key.x, 1.5);
        assert_eq!(key.y, 2.5);
        assert_eq!(key.center(), (2.0, 3.0));
    }

    #[test]
    fn layer_labels_fall_back_to_index() {
        let layer = Layer::default();
        assert_eq!(layer.label(2), "Layer 2");
        let layer = Layer {
            name: "Lower".into(),
            ..Default::default()
        };
        assert_eq!(layer.label(2), "Lower");
    }

    #[test]
    fn catalog_sorts_case_insensitively() {
        let mut catalog = Catalog::default();
        for (id, name) in [(1, "Momentary Layer"), (2, "Key Press"), (3, "transparent")] {
            catalog.insert(Behavior {
                id,
                display_name: name.into(),
                metadata: vec![],
            });
        }
        let names: Vec<&str> = catalog
            .sorted()
            .iter()
            .map(|behavior| behavior.display_name.as_str())
            .collect();
        assert_eq!(names, vec!["Key Press", "Momentary Layer", "transparent"]);
        assert_eq!(catalog.name(2), Some("Key Press"));
    }
}
