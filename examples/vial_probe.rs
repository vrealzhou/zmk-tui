//! Diagnostics probe for the Vial backend.
//!
//! Simulation (default, no hardware):
//!
//! ```bash
//! cargo run --example vial_probe
//! ```
//!
//! Real keyboard (read-only):
//!
//! ```bash
//! cargo run --example vial_probe -- --real --list
//! cargo run --example vial_probe -- --real
//! cargo run --example vial_probe -- --hid     # every HID interface, diagnostics
//! ```
//!
//! `--write-test` writes the existing keycode back to layer 0, position 0
//! (same value, no visible change) to verify the write path; `--restore FILE`
//! stages a `.vil` backup and saves it. Both are opt-in.

use std::process::ExitCode;

use zmk_tui::vial::RawHid;
use zmk_tui::vial::keycode::{Dialect, Translator, behavior};
use zmk_tui::vial::session::VialKeyboard;
use zmk_tui::vial::simulate::SimulatedVial;
use zmk_tui::vial::vil::VilDocument;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--real") {
        return real(&args);
    }
    if args.iter().any(|arg| arg == "--hid" || arg == "--list-all") {
        return hid_list();
    }
    if args.iter().any(|arg| arg == "--list") {
        return list();
    }
    simulate(&args)
}

fn simulate(args: &[String]) -> Result<(), String> {
    println!("Vial probe — in-process simulator (no hardware)");
    let mut keyboard =
        VialKeyboard::open(SimulatedVial::demo()).map_err(|error| error.to_string())?;
    describe(&keyboard);

    println!("\nUnlock gesture (simulated keys (0,0) and (0,1)):");
    let status = keyboard
        .unlock_status()
        .map_err(|error| error.to_string())?;
    println!("  locked={} keys={:?}", !status.unlocked, status.keys);
    keyboard.unlock_start().map_err(|error| error.to_string())?;
    keyboard.transport_mut().hold_keys(&status.keys);
    for _ in 0..3 {
        let polled = keyboard.unlock_poll().map_err(|error| error.to_string())?;
        println!(
            "  poll: unlocked={} in_progress={}",
            polled.unlocked, polled.in_progress
        );
    }
    keyboard.transport_mut().release_keys();

    println!("\nFirst layer:");
    print_layer(&keyboard, 0, 12);

    println!("\nStaging KC_B at layer 0, position 0 (KC_A -> KC_B):");
    let b = zmk_tui::model::Binding {
        behavior_id: behavior::KP,
        param1: 0x0007_0005,
        param2: 0,
    };
    keyboard
        .set_binding(0, 0, b)
        .map_err(|error| error.to_string())?;
    println!("  staged, unsaved={}", keyboard.has_unsaved_changes());
    keyboard.save().map_err(|error| error.to_string())?;
    println!(
        "  saved, keyboard now reports 0x{:04X}",
        keyboard.keycode_at(0, 0).unwrap_or(0)
    );

    let document = VilDocument::from_keyboard(&keyboard);
    let json = document
        .to_json_string()
        .map_err(|error| error.to_string())?;
    let path = std::env::temp_dir().join("zmk-tui-vial-backup.vil");
    std::fs::write(&path, json).map_err(|error| format!("write {}: {error}", path.display()))?;
    println!("\nBackup written to {}", path.display());

    if args.iter().any(|arg| arg == "--write-test") {
        // The simulator edit above already exercised the write path.
        println!("Simulator write test already ran.");
    }
    if let Some(path) = flag_value(args, "--restore") {
        let text =
            std::fs::read_to_string(&path).map_err(|error| format!("read {path}: {error}"))?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| format!("parse {path}: {error}"))?;
        let document =
            VilDocument::from_json(&value, keyboard.matrix()).map_err(|error| error.to_string())?;
        document
            .apply(&mut keyboard)
            .map_err(|error| error.to_string())?;
        keyboard.save().map_err(|error| error.to_string())?;
        println!("Restored {path}.");
    }

    keyboard.reset_keymap().map_err(|error| error.to_string())?;
    println!(
        "Reset simulator to stock: position 0 is 0x{:04X}",
        keyboard.keycode_at(0, 0).unwrap_or(0)
    );
    Ok(())
}

fn describe<T: RawHid>(keyboard: &VialKeyboard<T>) {
    let info = keyboard.info();
    println!(
        "  VIA 0x{:04X}, Vial {}, UID {}",
        info.via,
        info.vial.protocol,
        info.vial.uid_hex()
    );
    let definition = keyboard.definition();
    println!(
        "  definition \"{}\": {}x{} matrix, {} keys, {} encoder(s), {} duplicate cell(s)",
        definition.name,
        definition.matrix.rows,
        definition.matrix.cols,
        definition.keys.len(),
        definition.encoders,
        definition.duplicates
    );
    println!(
        "  layers: {}  lock: {}  unsaved: {}",
        keyboard.layer_count(),
        keyboard.lock_state().as_str(),
        keyboard.has_unsaved_changes()
    );
    let snapshot = keyboard.snapshot();
    for warning in &snapshot.warnings {
        println!("  warning: {warning}");
    }
}

fn print_layer<T: RawHid>(keyboard: &VialKeyboard<T>, layer: usize, count: usize) {
    let translator = Translator::new(Dialect::from_vial_protocol(keyboard.info().vial.protocol));
    let Some(bindings) = keyboard
        .keymap()
        .layers
        .get(layer)
        .map(|layer| layer.bindings.clone())
    else {
        println!("  (no layer {layer})");
        return;
    };
    for (index, binding) in bindings.iter().take(count).enumerate() {
        println!(
            "  {index:>2}: {}  (0x{:04X})",
            translator.label(*binding),
            keyboard.keycode_at(layer, index).unwrap_or(0)
        );
    }
}

fn hid_list() -> Result<(), String> {
    let devices = zmk_tui::vial::transport::list_all().map_err(|error| error.to_string())?;
    if devices.is_empty() {
        println!("No HID interfaces found.");
    }
    for device in devices {
        println!(
            "  {:04X}:{:04X} usage={:04X}/{:02X} vial_serial={} product={:?} serial={:?} path={}",
            device.vendor_id,
            device.product_id,
            device.usage_page,
            device.usage,
            device.is_vial,
            device.product,
            device.serial,
            device.path
        );
    }
    Ok(())
}

fn list() -> Result<(), String> {
    let devices = zmk_tui::vial::transport::list().map_err(|error| error.to_string())?;
    if devices.is_empty() {
        println!("No raw-HID interfaces (usage page 0xFF60 / usage 0x61) found.");
    }
    for device in devices {
        println!(
            "  {:04X}:{:04X} {} vial={} serial={:?} path={}",
            device.vendor_id,
            device.product_id,
            device.product,
            device.is_vial,
            device.serial,
            device.path
        );
    }
    Ok(())
}

fn real(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg == "--list") {
        return list();
    }
    let transport =
        zmk_tui::vial::transport::VialHid::open_first().map_err(|error| error.to_string())?;
    println!("Opened {}", transport.descriptor().path);
    let mut keyboard = VialKeyboard::open(transport).map_err(|error| error.to_string())?;
    describe(&keyboard);
    print_layer(&keyboard, 0, 12);

    let document = VilDocument::from_keyboard(&keyboard);
    let json = document
        .to_json_string()
        .map_err(|error| error.to_string())?;
    let path = std::env::temp_dir().join("zmk-tui-vial-backup.vil");
    std::fs::write(&path, json).map_err(|error| format!("write {}: {error}", path.display()))?;
    println!("\nRead-only backup written to {}", path.display());

    if args.iter().any(|arg| arg == "--write-test") {
        let current = keyboard
            .keycode_at(0, 0)
            .ok_or_else(|| "keyboard has no key at position 0".to_string())?;
        keyboard
            .stage_keycode(0, 0, current)
            .map_err(|error| error.to_string())?;
        keyboard.save().map_err(|error| error.to_string())?;
        println!("Write path verified (wrote 0x{current:04X} back).");
    }
    if let Some(path) = flag_value(args, "--restore") {
        let text =
            std::fs::read_to_string(&path).map_err(|error| format!("read {path}: {error}"))?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| format!("parse {path}: {error}"))?;
        let document =
            VilDocument::from_json(&value, keyboard.matrix()).map_err(|error| error.to_string())?;
        document
            .apply(&mut keyboard)
            .map_err(|error| error.to_string())?;
        keyboard.save().map_err(|error| error.to_string())?;
        println!("Restored {path}.");
    }
    Ok(())
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}
