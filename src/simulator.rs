//! An in-process ZMK Studio device used by tests and `zmk-tui --simulate`.
//!
//! It speaks the real wire protocol (framing + protobuf) through an
//! in-process duplex connection, so the client, UI and protocol code paths are
//! exercised exactly as they are with hardware.

use std::time::Duration;

use prost::Message;
use tokio::sync::mpsc;

use crate::keycodes;
use crate::model::{KeyGeometry, PhysicalLayout};
use crate::protocol::frame::Decoder;
use crate::protocol::messages::zmk;
use crate::transport::Connection;

/// A simulated keyboard model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SimulatedBoard {
    /// A 3x6+3 Corne, 42 keys.
    Corne,
    /// A Sofle with 6 columns per half and five thumb keys, 60 key positions.
    Sofle,
}

impl SimulatedBoard {
    pub fn label(self) -> &'static str {
        match self {
            Self::Corne => "Corne",
            Self::Sofle => "Sofle",
        }
    }

    pub fn device_name(self) -> &'static str {
        match self {
            Self::Corne => "Corne (simulator)",
            Self::Sofle => "Sofle (simulator)",
        }
    }

    pub fn key_count(self) -> usize {
        match self {
            Self::Corne => 42,
            Self::Sofle => 60,
        }
    }
}

const BEH_KEY_PRESS: u32 = 1001;
const BEH_MOD_TAP: u32 = 1002;
const BEH_LAYER_TAP: u32 = 1003;
const BEH_MO: u32 = 1004;
const BEH_TOG: u32 = 1005;
const BEH_TRANS: u32 = 1006;
const BEH_NONE: u32 = 1007;
const BEH_UNLOCK: u32 = 1008;
const BEH_STICKY: u32 = 1009;
const BEH_RESET: u32 = 1010;
const BEH_BOOTLOADER: u32 = 1011;
const BEH_OUTPUT: u32 = 1012;

const KEYBOARD_MAX: u32 = 0xE7;
const CONSUMER_MAX: u32 = 0x29D;
const MAX_LAYERS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimCommand {
    Lock,
    Unlock,
    Disconnect,
}

pub struct Simulator {
    commands: mpsc::UnboundedSender<SimCommand>,
    task: tokio::task::JoinHandle<()>,
}

impl Simulator {
    /// Start a simulator for `board` and return its client-side connection.
    pub fn start(board: SimulatedBoard, locked: bool) -> (Self, Connection) {
        let (connection, outgoing, incoming) = Connection::duplex(board.device_name());
        let (commands, command_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run(
            DeviceState::new(board, locked),
            outgoing,
            incoming,
            command_rx,
        ));
        (Self { commands, task }, connection)
    }

    pub fn lock(&self) {
        let _ = self.commands.send(SimCommand::Lock);
    }

    pub fn unlock(&self) {
        let _ = self.commands.send(SimCommand::Unlock);
    }

    pub fn disconnect(&self) {
        let _ = self.commands.send(SimCommand::Disconnect);
    }
}

impl Drop for Simulator {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A realistic 3x6+3 split layout for the simulator.
pub fn corne_geometry() -> PhysicalLayout {
    corne_geometry_with_gap(1.0, 3.25)
}

/// A second layout with a wider centre gap, useful for exercising the
/// physical-layout picker.
pub fn corne_wide_geometry() -> PhysicalLayout {
    corne_geometry_with_gap(2.0, 3.75)
}

fn corne_geometry_with_gap(gap: f32, thumb_offset: f32) -> PhysicalLayout {
    let mut keys = Vec::with_capacity(42);
    for row in 0..3 {
        for column in 0..12 {
            let x = if column < 6 {
                column as f32
            } else {
                column as f32 + gap
            };
            keys.push(KeyGeometry {
                x,
                y: row as f32,
                width: 1.0,
                height: 1.0,
                ..Default::default()
            });
        }
    }
    let left_thumbs = [thumb_offset, thumb_offset + 1.0, thumb_offset + 2.0];
    let right_thumbs = [thumb_offset + 3.5, thumb_offset + 4.5, thumb_offset + 5.5];
    for (index, x) in left_thumbs.into_iter().chain(right_thumbs).enumerate() {
        keys.push(KeyGeometry {
            x,
            y: 3.25,
            width: 1.0,
            height: 1.0,
            rotation: if index < 3 { -10.0 } else { 10.0 },
            rotation_x: x + 0.5,
            rotation_y: 3.75,
        });
    }
    PhysicalLayout {
        name: "Corne 3x6+3".into(),
        keys,
    }
}

/// The ZMK Sofle layout: four staggered rows per half, two encoder push
/// buttons between the halves, and five thumb keys per side (60 positions).
/// Rotations are included so the UI and navigation exercise realistic data.
pub fn sofle_geometry() -> PhysicalLayout {
    let left_x = [0.0_f32, 1.0, 2.0, 3.0, 4.0, 5.0];
    let right_x = [9.0_f32, 10.0, 11.0, 12.0, 13.0, 14.0];
    let left_stagger = [0.37_f32, 0.37, 0.12, 0.0, 0.12, 0.24];
    let right_stagger = [0.24_f32, 0.12, 0.0, 0.12, 0.37, 0.37];
    let mut keys = Vec::with_capacity(60);

    for row in 0..3 {
        for (column, x) in left_x.into_iter().enumerate() {
            keys.push(KeyGeometry {
                x,
                y: row as f32 + left_stagger[column],
                width: 1.0,
                height: 1.0,
                ..Default::default()
            });
        }
        for (column, x) in right_x.into_iter().enumerate() {
            keys.push(KeyGeometry {
                x,
                y: row as f32 + right_stagger[column],
                width: 1.0,
                height: 1.0,
                ..Default::default()
            });
        }
    }

    // Fourth row, including the two encoder push buttons in the middle.
    for (column, x) in left_x.into_iter().enumerate() {
        keys.push(KeyGeometry {
            x,
            y: 3.0 + left_stagger[column],
            width: 1.0,
            height: 1.0,
            ..Default::default()
        });
    }
    for x in [6.0_f32, 8.0] {
        keys.push(KeyGeometry {
            x,
            y: 2.74,
            width: 1.0,
            height: 1.0,
            ..Default::default()
        });
    }
    for (column, x) in right_x.into_iter().enumerate() {
        keys.push(KeyGeometry {
            x,
            y: 3.0 + right_stagger[column],
            width: 1.0,
            height: 1.0,
            ..Default::default()
        });
    }

    // Thumb clusters, matching the rotations in ZMK's Sofle layout.
    for (x, y, width, height, rotation, rx, ry) in [
        (
            1.75_f32, 4.37_f32, 1.0_f32, 1.0_f32, 0.0_f32, 0.0_f32, 0.0_f32,
        ),
        (2.75, 4.12, 1.0, 1.0, 0.0, 0.0, 0.0),
        (3.75, 4.12, 1.0, 1.0, 0.0, 0.0, 0.0),
        (4.90, 4.12, 1.0, 1.0, 12.0, 4.90, 4.12),
        (6.00, 3.83, 1.0, 1.5, 24.0, 6.00, 4.33),
        (8.00, 3.83, 1.0, 1.5, -24.0, 9.00, 4.33),
        (9.10, 4.12, 1.0, 1.0, -12.0, 10.10, 4.12),
        (10.25, 4.12, 1.0, 1.0, 0.0, 0.0, 0.0),
        (11.25, 4.12, 1.0, 1.0, 0.0, 0.0, 0.0),
        (12.25, 4.37, 1.0, 1.0, 0.0, 0.0, 0.0),
    ] {
        keys.push(KeyGeometry {
            x,
            y,
            width,
            height,
            rotation,
            rotation_x: rx,
            rotation_y: ry,
        });
    }

    PhysicalLayout {
        name: "Sofle".into(),
        keys,
    }
}

struct DeviceState {
    name: &'static str,
    locked: bool,
    layers: Vec<SimLayer>,
    stock_layers: Vec<SimLayer>,
    saved_layers: Vec<SimLayer>,
    unsaved: bool,
    active_layout: usize,
    layouts: Vec<PhysicalLayout>,
    key_count: usize,
}

#[derive(Clone)]
struct SimLayer {
    id: u32,
    name: String,
    bindings: Vec<zmk::keymap::BehaviorBinding>,
}

fn keycode(name: &str) -> u32 {
    keycodes::from_name(name)
        .map(|keycode| keycode.value)
        .unwrap_or(0)
}

fn binding(behavior_id: u32, param1: u32, param2: u32) -> zmk::keymap::BehaviorBinding {
    zmk::keymap::BehaviorBinding {
        behavior_id: behavior_id as i32,
        param1,
        param2,
    }
}

fn kp(name: &str) -> zmk::keymap::BehaviorBinding {
    binding(BEH_KEY_PRESS, keycode(name), 0)
}

fn mo(layer_id: u32) -> zmk::keymap::BehaviorBinding {
    binding(BEH_MO, layer_id, 0)
}

fn trans() -> zmk::keymap::BehaviorBinding {
    binding(BEH_TRANS, 0, 0)
}

fn none() -> zmk::keymap::BehaviorBinding {
    binding(BEH_NONE, 0, 0)
}

/// Pad a layer with `&trans` entries to the keyboard's key count.
fn pad(
    mut bindings: Vec<zmk::keymap::BehaviorBinding>,
    key_count: usize,
) -> Vec<zmk::keymap::BehaviorBinding> {
    bindings.resize(key_count, trans());
    bindings
}

fn f_keys() -> Vec<zmk::keymap::BehaviorBinding> {
    (13..=24).map(|number| kp(&format!("F{number}"))).collect()
}

fn media_keys() -> Vec<zmk::keymap::BehaviorBinding> {
    vec![
        kp("C_MUTE"),
        kp("C_VOLUME_DOWN"),
        kp("C_VOLUME_UP"),
        kp("C_PLAY_PAUSE"),
        kp("C_PREVIOUS"),
        kp("C_NEXT"),
        kp("C_STOP"),
        kp("C_EJECT"),
        kp("C_BRIGHTNESS_DEC"),
        kp("C_BRIGHTNESS_INC"),
        kp("K_POWER"),
        kp("GLOBE"),
    ]
}

fn corne_layers() -> Vec<SimLayer> {
    let base = SimLayer {
        id: 0,
        name: "Base".into(),
        bindings: vec![
            kp("ESCAPE"),
            kp("Q"),
            kp("W"),
            kp("E"),
            kp("R"),
            kp("T"),
            kp("Y"),
            kp("U"),
            kp("I"),
            kp("O"),
            kp("P"),
            kp("BSPC"),
            kp("TAB"),
            kp("A"),
            kp("S"),
            kp("D"),
            kp("F"),
            kp("G"),
            kp("H"),
            kp("J"),
            kp("K"),
            kp("L"),
            kp("SEMICOLON"),
            kp("SINGLE_QUOTE"),
            kp("LSHIFT"),
            kp("Z"),
            kp("X"),
            kp("C"),
            kp("V"),
            kp("B"),
            kp("N"),
            kp("M"),
            kp("COMMA"),
            kp("DOT"),
            kp("SLASH"),
            kp("RSHIFT"),
            kp("LCTRL"),
            kp("LGUI"),
            mo(1),
            mo(2),
            kp("LALT"),
            kp("SPACE"),
        ],
    };
    let lower = SimLayer {
        id: 1,
        name: "Lower".into(),
        bindings: vec![
            kp("GRAVE"),
            kp("NUMBER_1"),
            kp("NUMBER_2"),
            kp("NUMBER_3"),
            kp("NUMBER_4"),
            kp("NUMBER_5"),
            kp("NUMBER_6"),
            kp("NUMBER_7"),
            kp("NUMBER_8"),
            kp("NUMBER_9"),
            kp("NUMBER_0"),
            kp("MINUS"),
            trans(),
            kp("EXCLAMATION"),
            kp("AT_SIGN"),
            kp("HASH"),
            kp("DOLLAR"),
            kp("PERCENT"),
            kp("CARET"),
            kp("AMPERSAND"),
            kp("ASTERISK"),
            kp("LEFT_PARENTHESIS"),
            kp("RIGHT_PARENTHESIS"),
            kp("EQUAL"),
            trans(),
            kp("UNDERSCORE"),
            kp("PLUS"),
            kp("LEFT_BRACE"),
            kp("RIGHT_BRACE"),
            kp("PIPE"),
            kp("COLON"),
            kp("DOUBLE_QUOTES"),
            kp("LESS_THAN"),
            kp("GREATER_THAN"),
            kp("QUESTION"),
            kp("TILDE"),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
        ],
    };
    let mut raise = SimLayer {
        id: 2,
        name: "Raise".into(),
        bindings: vec![
            kp("F1"),
            kp("F2"),
            kp("F3"),
            kp("F4"),
            kp("F5"),
            kp("F6"),
            kp("F7"),
            kp("F8"),
            kp("F9"),
            kp("F10"),
            kp("F11"),
            kp("F12"),
            trans(),
            kp("INSERT"),
            kp("HOME"),
            kp("PAGE_UP"),
            kp("DELETE"),
            kp("END"),
            kp("PAGE_DOWN"),
            kp("LEFT_ARROW"),
            kp("DOWN_ARROW"),
            kp("UP_ARROW"),
            kp("RIGHT_ARROW"),
            kp("K_APPLICATION"),
            trans(),
            kp("C_VOLUME_DOWN"),
            kp("C_MUTE"),
            kp("C_VOLUME_UP"),
            kp("C_PREVIOUS"),
            kp("C_PLAY_PAUSE"),
            kp("C_NEXT"),
            kp("C_BRIGHTNESS_DEC"),
            kp("C_BRIGHTNESS_INC"),
            kp("K_POWER"),
            kp("K_CALCULATOR"),
            kp("GLOBE"),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
        ],
    };
    // Reachable from the Raise layer's thumb keys.
    raise.bindings[36] = mo(3);
    raise.bindings[41] = mo(4);

    let adjust = SimLayer {
        id: 3,
        name: "Adjust".into(),
        bindings: pad(f_keys(), 42),
    };
    let media = SimLayer {
        id: 4,
        name: "Media".into(),
        bindings: pad(media_keys(), 42),
    };
    vec![base, lower, raise, adjust, media]
}

fn sofle_layers() -> Vec<SimLayer> {
    let base = SimLayer {
        id: 0,
        name: "Base".into(),
        bindings: vec![
            kp("GRAVE"),
            kp("NUMBER_1"),
            kp("NUMBER_2"),
            kp("NUMBER_3"),
            kp("NUMBER_4"),
            kp("NUMBER_5"),
            kp("NUMBER_6"),
            kp("NUMBER_7"),
            kp("NUMBER_8"),
            kp("NUMBER_9"),
            kp("NUMBER_0"),
            none(),
            kp("ESCAPE"),
            kp("Q"),
            kp("W"),
            kp("E"),
            kp("R"),
            kp("T"),
            kp("Y"),
            kp("U"),
            kp("I"),
            kp("O"),
            kp("P"),
            kp("BSPC"),
            kp("TAB"),
            kp("A"),
            kp("S"),
            kp("D"),
            kp("F"),
            kp("G"),
            kp("H"),
            kp("J"),
            kp("K"),
            kp("L"),
            kp("SEMICOLON"),
            kp("SINGLE_QUOTE"),
            kp("LSHIFT"),
            kp("Z"),
            kp("X"),
            kp("C"),
            kp("V"),
            kp("B"),
            kp("C_MUTE"),
            none(),
            kp("N"),
            kp("M"),
            kp("COMMA"),
            kp("DOT"),
            kp("SLASH"),
            kp("RSHIFT"),
            kp("LGUI"),
            kp("LALT"),
            kp("LCTRL"),
            mo(1),
            kp("RETURN"),
            kp("SPACE"),
            mo(2),
            kp("RCTRL"),
            kp("RALT"),
            kp("RGUI"),
        ],
    };
    let lower = SimLayer {
        id: 1,
        name: "Lower".into(),
        bindings: vec![
            trans(),
            kp("F1"),
            kp("F2"),
            kp("F3"),
            kp("F4"),
            kp("F5"),
            kp("F6"),
            kp("F7"),
            kp("F8"),
            kp("F9"),
            kp("F10"),
            kp("F11"),
            kp("GRAVE"),
            kp("NUMBER_1"),
            kp("NUMBER_2"),
            kp("NUMBER_3"),
            kp("NUMBER_4"),
            kp("NUMBER_5"),
            kp("NUMBER_6"),
            kp("NUMBER_7"),
            kp("NUMBER_8"),
            kp("NUMBER_9"),
            kp("NUMBER_0"),
            kp("F12"),
            trans(),
            kp("EXCLAMATION"),
            kp("AT_SIGN"),
            kp("HASH"),
            kp("DOLLAR"),
            kp("PERCENT"),
            kp("CARET"),
            kp("AMPERSAND"),
            kp("ASTERISK"),
            kp("LEFT_PARENTHESIS"),
            kp("RIGHT_PARENTHESIS"),
            kp("PIPE"),
            trans(),
            kp("EQUAL"),
            kp("MINUS"),
            kp("PLUS"),
            kp("LEFT_BRACE"),
            kp("RIGHT_BRACE"),
            trans(),
            trans(),
            kp("LEFT_BRACKET"),
            kp("RIGHT_BRACKET"),
            kp("SEMICOLON"),
            kp("COLON"),
            kp("BACKSLASH"),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
        ],
    };
    let mut raise = SimLayer {
        id: 2,
        name: "Raise".into(),
        bindings: vec![
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            kp("INSERT"),
            kp("PRINTSCREEN"),
            kp("K_APPLICATION"),
            trans(),
            trans(),
            kp("PAGE_UP"),
            trans(),
            kp("UP_ARROW"),
            trans(),
            trans(),
            trans(),
            trans(),
            kp("LALT"),
            kp("LCTRL"),
            kp("LSHIFT"),
            trans(),
            kp("CAPSLOCK"),
            kp("PAGE_DOWN"),
            kp("LEFT_ARROW"),
            kp("DOWN_ARROW"),
            kp("RIGHT_ARROW"),
            kp("DELETE"),
            kp("BSPC"),
            trans(),
            kp("K_UNDO"),
            kp("K_CUT"),
            kp("K_COPY"),
            kp("K_PASTE"),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
            trans(),
        ],
    };
    // Reachable from the Raise layer's outer thumb keys.
    raise.bindings[50] = mo(3);
    raise.bindings[59] = mo(4);

    let adjust = SimLayer {
        id: 3,
        name: "Adjust".into(),
        bindings: pad(f_keys(), 60),
    };
    let media = SimLayer {
        id: 4,
        name: "Media".into(),
        bindings: pad(media_keys(), 60),
    };
    vec![base, lower, raise, adjust, media]
}

impl DeviceState {
    fn new(board: SimulatedBoard, locked: bool) -> Self {
        let layers = match board {
            SimulatedBoard::Corne => corne_layers(),
            SimulatedBoard::Sofle => sofle_layers(),
        };
        let layouts = match board {
            SimulatedBoard::Corne => vec![corne_geometry(), {
                let mut wide = corne_wide_geometry();
                wide.name = "Corne wide".into();
                wide
            }],
            SimulatedBoard::Sofle => vec![sofle_geometry()],
        };
        let key_count = layers
            .first()
            .map(|layer| layer.bindings.len())
            .unwrap_or(0);
        Self {
            name: board.device_name(),
            locked,
            stock_layers: layers.clone(),
            saved_layers: layers.clone(),
            layers,
            unsaved: false,
            active_layout: 0,
            layouts,
            key_count,
        }
    }

    fn keymap(&self) -> zmk::keymap::Keymap {
        zmk::keymap::Keymap {
            layers: self
                .layers
                .iter()
                .map(|layer| zmk::keymap::Layer {
                    id: layer.id,
                    name: layer.name.clone(),
                    bindings: layer.bindings.clone(),
                })
                .collect(),
            available_layers: MAX_LAYERS.saturating_sub(self.layers.len()) as u32,
            max_layer_name_length: 32,
        }
    }

    fn physical_layouts(&self) -> zmk::keymap::PhysicalLayouts {
        zmk::keymap::PhysicalLayouts {
            active_layout_index: self.active_layout as u32,
            layouts: self
                .layouts
                .iter()
                .map(|layout| zmk::keymap::PhysicalLayout {
                    name: layout.name.clone(),
                    keys: layout
                        .keys
                        .iter()
                        .map(|key| zmk::keymap::KeyPhysicalAttrs {
                            width: (key.width * 100.0) as i32,
                            height: (key.height * 100.0) as i32,
                            x: (key.x * 100.0) as i32,
                            y: (key.y * 100.0) as i32,
                            r: (key.rotation * 100.0) as i32,
                            rx: (key.rotation_x * 100.0) as i32,
                            ry: (key.rotation_y * 100.0) as i32,
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    fn behavior_details(&self, id: u32) -> Option<zmk::behaviors::GetBehaviorDetailsResponse> {
        let (display_name, metadata) = match id {
            BEH_KEY_PRESS => (
                "Key Press",
                vec![param_set(vec![hid_usage_param("Key")], vec![])],
            ),
            BEH_MOD_TAP => (
                "Mod-Tap",
                vec![param_set(
                    vec![hid_usage_param("Modifier")],
                    vec![hid_usage_param("Key")],
                )],
            ),
            BEH_LAYER_TAP => (
                "Layer-Tap",
                vec![param_set(
                    vec![layer_param("Layer")],
                    vec![hid_usage_param("Key")],
                )],
            ),
            BEH_MO => (
                "Momentary Layer",
                vec![param_set(vec![layer_param("Layer")], vec![])],
            ),
            BEH_TOG => (
                "Toggle Layer",
                vec![param_set(vec![layer_param("Layer")], vec![])],
            ),
            BEH_STICKY => (
                "Sticky Key",
                vec![param_set(vec![hid_usage_param("Key")], vec![])],
            ),
            BEH_TRANS => ("Transparent", vec![]),
            BEH_NONE => ("None", vec![]),
            BEH_UNLOCK => ("Studio Unlock", vec![]),
            BEH_RESET => ("Reset", vec![]),
            BEH_BOOTLOADER => ("Bootloader", vec![]),
            BEH_OUTPUT => ("Output Selection", vec![param_set(vec![], vec![])]),
            _ => return None,
        };
        Some(zmk::behaviors::GetBehaviorDetailsResponse {
            id,
            display_name: display_name.into(),
            metadata,
        })
    }

    fn list_behaviors(&self) -> Vec<u32> {
        [
            BEH_KEY_PRESS,
            BEH_MOD_TAP,
            BEH_LAYER_TAP,
            BEH_MO,
            BEH_TOG,
            BEH_STICKY,
            BEH_TRANS,
            BEH_NONE,
            BEH_UNLOCK,
            BEH_RESET,
            BEH_BOOTLOADER,
            BEH_OUTPUT,
        ]
        .to_vec()
    }
}

fn param_set(
    param1: Vec<zmk::behaviors::BehaviorParameterValueDescription>,
    param2: Vec<zmk::behaviors::BehaviorParameterValueDescription>,
) -> zmk::behaviors::BehaviorBindingParametersSet {
    zmk::behaviors::BehaviorBindingParametersSet { param1, param2 }
}

fn hid_usage_param(name: &str) -> zmk::behaviors::BehaviorParameterValueDescription {
    zmk::behaviors::BehaviorParameterValueDescription {
        name: name.into(),
        value_type: Some(
            zmk::behaviors::behavior_parameter_value_description::ValueType::HidUsage(
                zmk::behaviors::BehaviorParameterHidUsage {
                    keyboard_max: KEYBOARD_MAX,
                    consumer_max: CONSUMER_MAX,
                },
            ),
        ),
    }
}

fn layer_param(name: &str) -> zmk::behaviors::BehaviorParameterValueDescription {
    zmk::behaviors::BehaviorParameterValueDescription {
        name: name.into(),
        value_type: Some(
            zmk::behaviors::behavior_parameter_value_description::ValueType::LayerId(
                zmk::behaviors::BehaviorParameterLayerId {},
            ),
        ),
    }
}

fn meta_error(code: zmk::meta::ErrorConditions) -> zmk::studio::RequestResponse {
    zmk::studio::RequestResponse {
        request_id: 0,
        subsystem: Some(zmk::studio::request_response::Subsystem::Meta(
            zmk::meta::Response {
                response_type: Some(zmk::meta::response::ResponseType::SimpleError(code as i32)),
            },
        )),
    }
}

fn no_response() -> zmk::studio::RequestResponse {
    zmk::studio::RequestResponse {
        request_id: 0,
        subsystem: Some(zmk::studio::request_response::Subsystem::Meta(
            zmk::meta::Response {
                response_type: Some(zmk::meta::response::ResponseType::NoResponse(true)),
            },
        )),
    }
}

async fn run(
    mut device: DeviceState,
    mut outgoing: mpsc::UnboundedReceiver<Vec<u8>>,
    incoming: mpsc::UnboundedSender<Result<Vec<u8>, String>>,
    mut commands: mpsc::UnboundedReceiver<SimCommand>,
) {
    let mut decoder = Decoder::new();
    let mut frames = Vec::new();

    loop {
        tokio::select! {
            command = commands.recv() => {
                match command {
                    Some(SimCommand::Lock) => {
                        if !device.locked {
                            device.locked = true;
                            send_notification(&incoming, core_lock_notification(true));
                        }
                    }
                    Some(SimCommand::Unlock) => {
                        if device.locked {
                            device.locked = false;
                            send_notification(&incoming, core_lock_notification(false));
                        }
                    }
                    Some(SimCommand::Disconnect) | None => {
                        let _ = incoming.send(Err("simulator disconnected".into()));
                        return;
                    }
                }
            }
            bytes = outgoing.recv() => {
                let Some(bytes) = bytes else { return; };
                frames.clear();
                if decoder.feed_slice(&bytes, &mut frames).is_err() {
                    continue;
                }
                for frame in frames.drain(..) {
                    let Ok(request) = zmk::studio::Request::decode(frame.as_slice()) else {
                        continue;
                    };
                    let request_id = request.request_id;
                    let mut response = handle_request(&mut device, request);
                    response.request_id = request_id;
                    let payload = zmk::studio::Response {
                        r#type: Some(zmk::studio::response::Type::RequestResponse(response)),
                    }
                    .encode_to_vec();
                    if incoming.send(Ok(payload)).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

fn send_notification(
    incoming: &mpsc::UnboundedSender<Result<Vec<u8>, String>>,
    notification: zmk::studio::Notification,
) {
    let payload = zmk::studio::Response {
        r#type: Some(zmk::studio::response::Type::Notification(notification)),
    }
    .encode_to_vec();
    let _ = incoming.send(Ok(payload));
}

fn core_lock_notification(locked: bool) -> zmk::studio::Notification {
    zmk::studio::Notification {
        subsystem: Some(zmk::studio::notification::Subsystem::Core(
            zmk::core::Notification {
                notification_type: Some(
                    zmk::core::notification::NotificationType::LockStateChanged(if locked {
                        0
                    } else {
                        1
                    }),
                ),
            },
        )),
    }
}

fn is_secured(subsystem: &zmk::studio::request::Subsystem) -> bool {
    match subsystem {
        zmk::studio::request::Subsystem::Core(core) => !matches!(
            core.request_type,
            Some(
                zmk::core::request::RequestType::GetDeviceInfo(_)
                    | zmk::core::request::RequestType::GetLockState(_)
            )
        ),
        zmk::studio::request::Subsystem::Behaviors(behaviors) => !matches!(
            behaviors.request_type,
            Some(zmk::behaviors::request::RequestType::ListAllBehaviors(_))
        ),
        zmk::studio::request::Subsystem::Keymap(_) => true,
    }
}

fn handle_request(
    device: &mut DeviceState,
    request: zmk::studio::Request,
) -> zmk::studio::RequestResponse {
    let Some(subsystem) = request.subsystem else {
        return no_response();
    };
    let secured = is_secured(&subsystem);
    if secured && device.locked {
        return meta_error(zmk::meta::ErrorConditions::UnlockRequired);
    }

    match subsystem {
        zmk::studio::request::Subsystem::Core(core) => match core.request_type {
            Some(zmk::core::request::RequestType::GetDeviceInfo(_)) => {
                core_response(zmk::core::response::ResponseType::GetDeviceInfo(
                    zmk::core::GetDeviceInfoResponse {
                        name: device.name.into(),
                        serial_number: b"SIM-0001".to_vec(),
                    },
                ))
            }
            Some(zmk::core::request::RequestType::GetLockState(_)) => core_response(
                zmk::core::response::ResponseType::GetLockState(if device.locked { 0 } else { 1 }),
            ),
            Some(zmk::core::request::RequestType::ResetSettings(_)) => {
                device.layers = device.stock_layers.clone();
                device.saved_layers = device.stock_layers.clone();
                device.active_layout = 0;
                device.unsaved = false;
                core_response(zmk::core::response::ResponseType::ResetSettings(true))
            }
            _ => no_response(),
        },
        zmk::studio::request::Subsystem::Behaviors(behaviors) => match behaviors.request_type {
            Some(zmk::behaviors::request::RequestType::ListAllBehaviors(_)) => {
                behaviors_response(zmk::behaviors::response::ResponseType::ListAllBehaviors(
                    zmk::behaviors::ListAllBehaviorsResponse {
                        behaviors: device.list_behaviors(),
                    },
                ))
            }
            Some(zmk::behaviors::request::RequestType::GetBehaviorDetails(request)) => {
                match device.behavior_details(request.behavior_id) {
                    Some(details) => behaviors_response(
                        zmk::behaviors::response::ResponseType::GetBehaviorDetails(details),
                    ),
                    None => meta_error(zmk::meta::ErrorConditions::Generic),
                }
            }
            None => no_response(),
        },
        zmk::studio::request::Subsystem::Keymap(keymap) => handle_keymap(device, keymap),
    }
}

fn handle_keymap(
    device: &mut DeviceState,
    request: zmk::keymap::Request,
) -> zmk::studio::RequestResponse {
    use zmk::keymap::request::RequestType;

    match request.request_type {
        Some(RequestType::GetKeymap(_)) => keymap_response(
            zmk::keymap::response::ResponseType::GetKeymap(device.keymap()),
        ),
        Some(RequestType::GetPhysicalLayouts(_)) => keymap_response(
            zmk::keymap::response::ResponseType::GetPhysicalLayouts(device.physical_layouts()),
        ),
        Some(RequestType::CheckUnsavedChanges(_)) => keymap_response(
            zmk::keymap::response::ResponseType::CheckUnsavedChanges(device.unsaved),
        ),
        Some(RequestType::SetLayerBinding(set)) => {
            let Some(layer_index) = device
                .layers
                .iter()
                .position(|layer| layer.id == set.layer_id)
            else {
                return keymap_response(zmk::keymap::response::ResponseType::SetLayerBinding(
                    zmk::keymap::SetLayerBindingResponse::SetLayerBindingRespInvalidLocation as i32,
                ));
            };
            let Some(binding) = set.binding else {
                return keymap_response(zmk::keymap::response::ResponseType::SetLayerBinding(
                    zmk::keymap::SetLayerBindingResponse::SetLayerBindingRespInvalidParameters
                        as i32,
                ));
            };
            let position = set.key_position.max(0) as usize;
            if position >= device.layers[layer_index].bindings.len() {
                return keymap_response(zmk::keymap::response::ResponseType::SetLayerBinding(
                    zmk::keymap::SetLayerBindingResponse::SetLayerBindingRespInvalidLocation as i32,
                ));
            }
            if device
                .behavior_details(binding.behavior_id.max(0) as u32)
                .is_none()
            {
                return keymap_response(zmk::keymap::response::ResponseType::SetLayerBinding(
                    zmk::keymap::SetLayerBindingResponse::SetLayerBindingRespInvalidBehavior as i32,
                ));
            }
            device.layers[layer_index].bindings[position] = binding;
            device.unsaved = true;
            keymap_response(zmk::keymap::response::ResponseType::SetLayerBinding(
                zmk::keymap::SetLayerBindingResponse::SetLayerBindingRespOk as i32,
            ))
        }
        Some(RequestType::SaveChanges(_)) => {
            device.saved_layers = device.layers.clone();
            device.unsaved = false;
            keymap_response(zmk::keymap::response::ResponseType::SaveChanges(
                zmk::keymap::SaveChangesResponse {
                    result: Some(zmk::keymap::save_changes_response::Result::Ok(true)),
                },
            ))
        }
        Some(RequestType::DiscardChanges(_)) => {
            device.layers = device.saved_layers.clone();
            device.unsaved = false;
            keymap_response(zmk::keymap::response::ResponseType::DiscardChanges(true))
        }
        Some(RequestType::AddLayer(_)) => {
            if device.layers.len() >= MAX_LAYERS {
                return keymap_response(zmk::keymap::response::ResponseType::AddLayer(
                    zmk::keymap::AddLayerResponse {
                        result: Some(zmk::keymap::add_layer_response::Result::Err(
                            zmk::keymap::AddLayerErrorCode::AddLayerErrNoSpace as i32,
                        )),
                    },
                ));
            }
            let id = device
                .layers
                .iter()
                .map(|layer| layer.id)
                .max()
                .unwrap_or(0)
                + 1;
            let index = device.layers.len() as u32;
            device.layers.push(SimLayer {
                id,
                name: String::new(),
                bindings: vec![trans(); device.key_count],
            });
            device.unsaved = true;
            keymap_response(zmk::keymap::response::ResponseType::AddLayer(
                zmk::keymap::AddLayerResponse {
                    result: Some(zmk::keymap::add_layer_response::Result::Ok(
                        zmk::keymap::AddLayerResponseDetails {
                            index,
                            layer: Some(zmk::keymap::Layer {
                                id,
                                name: String::new(),
                                bindings: vec![trans(); device.key_count],
                            }),
                        },
                    )),
                },
            ))
        }
        Some(RequestType::RemoveLayer(remove)) => {
            let index = remove.layer_index as usize;
            if index >= device.layers.len() || device.layers.len() <= 1 {
                return keymap_response(zmk::keymap::response::ResponseType::RemoveLayer(
                    zmk::keymap::RemoveLayerResponse {
                        result: Some(zmk::keymap::remove_layer_response::Result::Err(
                            zmk::keymap::RemoveLayerErrorCode::RemoveLayerErrInvalidIndex as i32,
                        )),
                    },
                ));
            }
            device.layers.remove(index);
            device.unsaved = true;
            keymap_response(zmk::keymap::response::ResponseType::RemoveLayer(
                zmk::keymap::RemoveLayerResponse {
                    result: Some(zmk::keymap::remove_layer_response::Result::Ok(
                        zmk::keymap::RemoveLayerOk {},
                    )),
                },
            ))
        }
        Some(RequestType::MoveLayer(move_layer)) => {
            let start = move_layer.start_index as usize;
            let dest = move_layer.dest_index as usize;
            if start >= device.layers.len() || dest >= device.layers.len() {
                return keymap_response(zmk::keymap::response::ResponseType::MoveLayer(
                    zmk::keymap::MoveLayerResponse {
                        result: Some(zmk::keymap::move_layer_response::Result::Err(
                            zmk::keymap::MoveLayerErrorCode::MoveLayerErrInvalidLayer as i32,
                        )),
                    },
                ));
            }
            let layer = device.layers.remove(start);
            device.layers.insert(dest, layer);
            device.unsaved = true;
            keymap_response(zmk::keymap::response::ResponseType::MoveLayer(
                zmk::keymap::MoveLayerResponse {
                    result: Some(zmk::keymap::move_layer_response::Result::Ok(
                        device.keymap(),
                    )),
                },
            ))
        }
        Some(RequestType::SetLayerProps(props)) => {
            match device
                .layers
                .iter_mut()
                .find(|layer| layer.id == props.layer_id)
            {
                Some(layer) => {
                    layer.name = props.name;
                    device.unsaved = true;
                    keymap_response(zmk::keymap::response::ResponseType::SetLayerProps(
                        zmk::keymap::SetLayerPropsResponse::SetLayerPropsRespOk as i32,
                    ))
                }
                None => keymap_response(zmk::keymap::response::ResponseType::SetLayerProps(
                    zmk::keymap::SetLayerPropsResponse::SetLayerPropsRespErrInvalidId as i32,
                )),
            }
        }
        Some(RequestType::SetActivePhysicalLayout(index)) => {
            if (index as usize) < device.layouts.len() {
                device.active_layout = index as usize;
                device.unsaved = true;
                keymap_response(
                    zmk::keymap::response::ResponseType::SetActivePhysicalLayout(
                        zmk::keymap::SetActivePhysicalLayoutResponse {
                            result: Some(
                                zmk::keymap::set_active_physical_layout_response::Result::Ok(
                                    device.keymap(),
                                ),
                            ),
                        },
                    ),
                )
            } else {
                keymap_response(
                    zmk::keymap::response::ResponseType::SetActivePhysicalLayout(
                        zmk::keymap::SetActivePhysicalLayoutResponse {
                            result: Some(
                                zmk::keymap::set_active_physical_layout_response::Result::Err(
                                    zmk::keymap::SetActivePhysicalLayoutErrorCode::SetActivePhysicalLayoutErrInvalidLayoutIndex as i32,
                                ),
                            ),
                        },
                    ),
                )
            }
        }
        Some(RequestType::RestoreLayer(_)) | None => no_response(),
    }
}

fn core_response(response_type: zmk::core::response::ResponseType) -> zmk::studio::RequestResponse {
    zmk::studio::RequestResponse {
        request_id: 0,
        subsystem: Some(zmk::studio::request_response::Subsystem::Core(
            zmk::core::Response {
                response_type: Some(response_type),
            },
        )),
    }
}

fn behaviors_response(
    response_type: zmk::behaviors::response::ResponseType,
) -> zmk::studio::RequestResponse {
    zmk::studio::RequestResponse {
        request_id: 0,
        subsystem: Some(zmk::studio::request_response::Subsystem::Behaviors(
            zmk::behaviors::Response {
                response_type: Some(response_type),
            },
        )),
    }
}

fn keymap_response(
    response_type: zmk::keymap::response::ResponseType,
) -> zmk::studio::RequestResponse {
    zmk::studio::RequestResponse {
        request_id: 0,
        subsystem: Some(zmk::studio::request_response::Subsystem::Keymap(
            zmk::keymap::Response {
                response_type: Some(response_type),
            },
        )),
    }
}

/// Give the simulator's async tasks a chance to observe a command.
pub async fn settle() {
    tokio::time::sleep(Duration::from_millis(10)).await;
}
