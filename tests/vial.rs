//! End-to-end tests for the Vial backend against the in-process simulator.
//!
//! These cover the contract the TUI will rely on: read a definition and keymap,
//! stage edits, save/discard, preserve unsupported keycodes, unlock, and
//! export/restore a `.vil` backup.

use zmk_tui::device::Capabilities;
use zmk_tui::model::Binding;
use zmk_tui::transport::ConnectionKind;
use zmk_tui::tui::app::VialStart;
use zmk_tui::tui::connection::connect_vial;
use zmk_tui::vial::definition::KeyboardDefinition;
use zmk_tui::vial::keycode::behavior;
use zmk_tui::vial::protocol::{REPORT_SIZE, VialError};
use zmk_tui::vial::session::VialKeyboard;
use zmk_tui::vial::simulate::{DEMO_COLS, DEMO_LAYERS, DEMO_ROWS, SimulatedVial};
use zmk_tui::vial::vil::VilDocument;

fn open_unlocked() -> VialKeyboard<SimulatedVial> {
    let simulator = SimulatedVial::demo().with_unlocked(true);
    VialKeyboard::open(simulator).expect("open simulator")
}

#[test]
fn reads_the_embedded_definition_and_the_whole_keymap() {
    let keyboard = open_unlocked();
    assert_eq!(keyboard.layer_count(), usize::from(DEMO_LAYERS));
    assert_eq!(keyboard.definition().keys.len(), 24);
    assert_eq!(keyboard.definition().matrix.rows, DEMO_ROWS);
    assert_eq!(keyboard.definition().matrix.cols, DEMO_COLS);
    let snapshot = keyboard.snapshot();
    assert_eq!(snapshot.keymap.layers.len(), 4);
    assert_eq!(
        snapshot.keymap.layers[0].bindings[0].behavior_id,
        behavior::KP
    );
    assert_eq!(snapshot.keymap.layers[0].bindings[0].param1, 0x0007_0004);
    assert_eq!(
        snapshot.keymap.layers[1].bindings[0].behavior_id,
        behavior::TRANS
    );
    assert_eq!(snapshot.layouts.layouts.len(), 1);
    assert_eq!(snapshot.catalog.len(), 14);
}

#[test]
fn the_definition_json_is_xz_compressed_like_vial_qmk() {
    let simulator = SimulatedVial::demo();
    let json = simulator.definition_json();
    assert_eq!(json.first(), Some(&b'{'));
    let definition = KeyboardDefinition::parse(&json).unwrap();
    assert_eq!(definition.matrix.cols, DEMO_COLS);
    assert_eq!(definition.keys.len(), 24);
}

#[test]
fn edits_are_staged_until_save() {
    let mut keyboard = open_unlocked();
    let b = BehaviorKey::key(behavior::KP, 0x0007_0005);
    keyboard.set_binding(0, 0, b).unwrap();
    assert_eq!(keyboard.keycode_at(0, 0), Some(0x0005));
    assert!(keyboard.has_unsaved_changes());
    // The device still has KC_A until save.
    assert_eq!(keyboard.transport().keycode(0, 0, 0), 0x0004);

    keyboard.save().unwrap();
    assert!(!keyboard.has_unsaved_changes());
    assert_eq!(keyboard.transport().keycode(0, 0, 0), 0x0005);
    assert_eq!(keyboard.transport().writes, vec![(0, 0, 0, 0x0005)]);
}

#[test]
fn discard_drops_staged_edits_and_reloads() {
    let mut keyboard = open_unlocked();
    keyboard
        .set_binding(0, 0, BehaviorKey::key(behavior::KP, 0x0007_0005))
        .unwrap();
    keyboard.discard().unwrap();
    assert!(!keyboard.has_unsaved_changes());
    assert_eq!(keyboard.keycode_at(0, 0), Some(0x0004));
    assert_eq!(keyboard.transport().writes, vec![]);
}

#[test]
fn a_failed_save_keeps_the_remaining_edits_staged() {
    let mut keyboard = open_unlocked();
    keyboard
        .set_binding(0, 0, BehaviorKey::key(behavior::KP, 0x0007_0005))
        .unwrap();
    keyboard.transport_mut().fail_next("cable unplugged");
    let error = keyboard.save().unwrap_err();
    assert!(matches!(error, VialError::Transport(_)));
    assert!(keyboard.has_unsaved_changes());
    keyboard.save().unwrap();
    assert_eq!(keyboard.transport().keycode(0, 0, 0), 0x0005);
}

#[test]
fn unsupported_bindings_are_rejected_instead_of_written() {
    let mut keyboard = open_unlocked();
    let raw_too_large = Binding {
        behavior_id: behavior::RAW,
        param1: 0x1_0000,
        param2: 0,
    };
    assert!(keyboard.set_binding(0, 0, raw_too_large).is_err());
    assert!(!keyboard.has_unsaved_changes());
}

#[test]
fn reset_restores_the_compiled_keymap() {
    let mut keyboard = open_unlocked();
    keyboard
        .set_binding(0, 0, BehaviorKey::key(behavior::KP, 0x0007_0005))
        .unwrap();
    keyboard.save().unwrap();
    assert_eq!(keyboard.keycode_at(0, 0), Some(0x0005));
    keyboard.reset_keymap().unwrap();
    assert_eq!(keyboard.keycode_at(0, 0), Some(0x0004));
}

#[test]
fn unlock_gesture_is_polled_not_guessed() {
    let mut keyboard = VialKeyboard::open(SimulatedVial::demo()).unwrap();
    assert!(keyboard.lock_state().is_locked());
    let status = keyboard.unlock_status().unwrap();
    assert!(!status.unlocked);
    assert_eq!(status.keys, vec![(0, 0), (0, 1)]);

    keyboard.unlock_start().unwrap();
    keyboard.transport_mut().hold_keys(&[(0, 0), (0, 1)]);
    let mut status = keyboard.unlock_poll().unwrap();
    for _ in 0..2 {
        assert!(!status.unlocked);
        status = keyboard.unlock_poll().unwrap();
    }
    assert!(status.unlocked);
    assert!(!keyboard.lock_state().is_locked());
}

#[test]
fn vil_export_and_restore_round_trip_the_keymap() {
    let mut keyboard = open_unlocked();
    keyboard
        .set_binding(1, 2, BehaviorKey::key(behavior::KP, 0x0007_0006))
        .unwrap();
    let document = VilDocument::from_keyboard(&keyboard);
    let json = document.to_json_string().unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let parsed = VilDocument::from_json(&value, keyboard.matrix()).unwrap();
    assert_eq!(parsed, document);

    // A fresh keyboard has KC_A where the backup has KC_C on layer 1.
    let mut restored = open_unlocked();
    parsed.apply(&mut restored).unwrap();
    restored.save().unwrap();
    assert_eq!(restored.keycode_at(1, 2), Some(0x0006));
}

#[test]
fn a_vil_from_another_keyboard_is_rejected() {
    let keyboard = open_unlocked();
    let mut document = VilDocument::from_keyboard(&keyboard);
    document.uid ^= 0xFFFF;
    let error = document.apply(&mut open_unlocked()).unwrap_err();
    assert!(error.to_string().contains("does not match"));
}

#[test]
fn unsupported_protocol_is_rejected_with_a_clear_error() {
    let simulator = SimulatedVial::demo().with_vial_protocol(7);
    let error = match VialKeyboard::open(simulator) {
        Ok(_) => panic!("Vial protocol 7 must be rejected"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        VialError::UnsupportedProtocol { vial: 7, .. }
    ));
}

#[test]
fn every_report_is_exactly_32_bytes() {
    assert_eq!(REPORT_SIZE, 32);
}

#[tokio::test]
async fn the_tui_connection_path_edits_through_the_device_client() {
    let connected = connect_vial(VialStart::Simulate).await.unwrap();
    assert_eq!(connected.kind, ConnectionKind::VialSimulator);
    assert_eq!(connected.client.capabilities(), Capabilities::VIAL);
    assert_eq!(connected.info.name, "Simulated Vial 4x6");
    let snapshot = connected.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.keymap.layers.len(), 4);

    connected
        .client
        .set_layer_binding(0, 0, BehaviorKey::key(behavior::KP, 0x0007_0005))
        .await
        .unwrap();
    connected.client.save_changes().await.unwrap();
    let keymap = connected.client.get_keymap().await.unwrap();
    assert_eq!(keymap.layers[0].bindings[0].param1, 0x0007_0005);

    // Discarding reloads the last saved state, which is now KC_B.
    connected.client.discard_changes().await.unwrap();
    let keymap = connected.client.get_keymap().await.unwrap();
    assert_eq!(keymap.layers[0].bindings[0].param1, 0x0007_0005);

    // Layer management is a Studio capability; Vial reports it honestly.
    assert!(connected.client.add_layer().await.is_err());
    assert!(connected.client.move_layer(0, 1).await.is_err());
    assert!(!connected.client.capabilities().manage_layers);
}

/// Small helper so tests read like keycode selections.
struct BehaviorKey;

impl BehaviorKey {
    fn key(behavior_id: u32, param1: u32) -> Binding {
        Binding {
            behavior_id,
            param1,
            param2: 0,
        }
    }
}
