//! End-to-end tests for the interactive UI state machine.
//!
//! Each test drives `App` exactly as the event loop does and renders the real
//! widget tree into a `TestBackend`, so key handling and drawing are covered
//! without needing a physical terminal.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tokio::sync::mpsc::{self, UnboundedReceiver};

use zmk_tui::client::{StudioError, StudioEvent};
use zmk_tui::keycodes;
use zmk_tui::model::{Binding, LockState};
use zmk_tui::simulator::SimulatedBoard;
use zmk_tui::transport::serial::UsbDevice;
use zmk_tui::tui::StartOptions;
use zmk_tui::tui::app::{App, Msg, Panel, StatusKind};
use zmk_tui::tui::connection::Snapshot;
use zmk_tui::tui::modals::{Modal, Stage, UnlockModal, behavior_items};
use zmk_tui::tui::mouse::{ListKind, MouseTarget};
use zmk_tui::tui::render;

fn screen_text(app: &mut App) -> String {
    screen_text_sized(app, 120, 35)
}

fn screen_text_sized(app: &mut App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render::render(frame, app))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn press(app: &mut App, code: KeyCode) {
    app.handle(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn type_text(app: &mut App, text: &str) {
    for ch in text.chars() {
        press(app, KeyCode::Char(ch));
    }
}

fn click_at(app: &mut App, column: u16, row: u16) {
    app.handle(Msg::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }));
}

fn click_target(app: &mut App, target: MouseTarget) {
    let rect = app
        .mouse
        .target_rect(target)
        .unwrap_or_else(|| panic!("no mouse target {target:?}"));
    click_at(app, rect.x + rect.width / 2, rect.y + rect.height / 2);
}

fn click_list(app: &mut App, kind: ListKind, index: usize) {
    let rect = app
        .mouse
        .item_rect(kind, index)
        .unwrap_or_else(|| panic!("no list item {kind:?}[{index}]"));
    click_at(app, rect.x + 1, rect.y);
}

fn scroll_list(app: &mut App, kind: ListKind, down: bool) {
    let list = app.mouse.list(kind).expect("list");
    app.handle(Msg::Mouse(MouseEvent {
        kind: if down {
            MouseEventKind::ScrollDown
        } else {
            MouseEventKind::ScrollUp
        },
        column: list.area.x + 1,
        row: list.area.y,
        modifiers: KeyModifiers::NONE,
    }));
}

async fn pump_until(
    app: &mut App,
    messages: &mut UnboundedReceiver<Msg>,
    what: &str,
    predicate: impl Fn(&App) -> bool,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if predicate(app) {
            return;
        }
        let message = tokio::time::timeout_at(deadline, messages.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
            .expect("message channel closed");
        app.handle(message);
    }
}

async fn connected_app_for(board: SimulatedBoard) -> (App, UnboundedReceiver<Msg>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = App::new(
        tx,
        StartOptions {
            simulate: Some(board),
            ..Default::default()
        },
    );
    app.on_start();
    pump_until(&mut app, &mut rx, "the simulator keymap", |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.keymap_loaded)
            .unwrap_or(false)
    })
    .await;
    (app, rx)
}

async fn connected_app() -> (App, UnboundedReceiver<Msg>) {
    connected_app_for(SimulatedBoard::Corne).await
}

fn disconnected_app() -> App {
    let (tx, _rx) = mpsc::unbounded_channel();
    App::new(tx, StartOptions::default())
}

#[tokio::test]
async fn connects_renders_and_quits() {
    let (mut app, _rx) = connected_app().await;

    let editor = app.editor.as_ref().expect("editor");
    assert_eq!(editor.info.name, "Corne (simulator)");
    assert_eq!(editor.catalog.len(), 12);
    assert_eq!(editor.layouts.layouts[0].keys.len(), 42);

    let screen = screen_text(&mut app);
    for expected in [
        "Corne (simulator)",
        "Base",
        "Lower",
        "Raise",
        "Adjust",
        "Media",
        "ESC",
        "Q",
        "Connected to Corne",
        "&kp ESCAPE",
        "Corne 3x6+3",
    ] {
        assert!(
            screen.contains(expected),
            "missing {expected:?} in:\n{screen}"
        );
    }

    press(&mut app, KeyCode::Esc);
    let cleared = screen_text(&mut app);
    assert!(cleared.contains("Enter edit"), "{cleared}");

    press(&mut app, KeyCode::Char('?'));
    let help = screen_text(&mut app);
    assert!(help.contains("Navigation"));
    assert!(help.contains("save changes"));
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char('q'));
    assert!(app.should_quit);
}

#[tokio::test]
async fn edits_a_binding_and_saves_it() {
    let (mut app, mut rx) = connected_app().await;
    let a_value = keycodes::from_name("A").unwrap().value;

    press(&mut app, KeyCode::Enter);
    let binding_screen = screen_text(&mut app);
    assert!(binding_screen.contains("Edit binding"));
    assert!(binding_screen.contains("Choose the behavior"));

    type_text(&mut app, "key press");
    press(&mut app, KeyCode::Enter);
    let param_screen = screen_text(&mut app);
    assert!(param_screen.contains("Parameter 1"), "{param_screen}");
    assert!(param_screen.contains("Key"), "{param_screen}");

    type_text(&mut app, "a");
    press(&mut app, KeyCode::Enter);
    let review = screen_text(&mut app);
    assert!(review.contains("Review"), "{review}");
    assert!(review.contains("&kp A"), "{review}");

    press(&mut app, KeyCode::Enter);
    pump_until(&mut app, &mut rx, "the binding update", |app| {
        app.editor
            .as_ref()
            .map(|editor| {
                editor.keymap.layers[0].bindings[0].param1 == a_value
                    && editor.dirty
                    && app.busy.is_none()
            })
            .unwrap_or(false)
    })
    .await;

    let applied = screen_text(&mut app);
    assert!(applied.contains("UNSAVED"), "{applied}");
    assert!(applied.contains("&kp A"), "{applied}");

    press(&mut app, KeyCode::Char('s'));
    pump_until(&mut app, &mut rx, "the save", |app| {
        app.editor
            .as_ref()
            .map(|editor| !editor.dirty && app.busy.is_none())
            .unwrap_or(false)
    })
    .await;
    assert!(screen_text(&mut app).contains("Saved to keyboard"));

    press(&mut app, KeyCode::Char('q'));
    assert!(app.should_quit);
}

#[tokio::test]
async fn sofle_renders_sixty_positions_and_five_layers() {
    let (mut app, _rx) = connected_app_for(SimulatedBoard::Sofle).await;
    let editor = app.editor.as_ref().expect("editor");

    assert_eq!(editor.info.name, "Sofle (simulator)");
    assert_eq!(editor.keymap.layers.len(), 5);
    assert_eq!(editor.position_count(), 60);
    assert_eq!(editor.layouts.layouts.len(), 1);
    assert_eq!(editor.layouts.layouts[0].keys.len(), 60);
    assert_eq!(
        editor.keymap.layers[0].bindings[0].param1,
        keycodes::from_name("GRAVE").unwrap().value
    );

    let screen = screen_text(&mut app);
    for expected in [
        "Sofle (simulator)",
        "Base",
        "Lower",
        "Raise",
        "Adjust",
        "Media",
        "LCTL",
        "RGUI",
        "GRAVE",
    ] {
        assert!(
            screen.contains(expected),
            "missing {expected:?} in:\n{screen}"
        );
    }

    // The last thumb key is RGUI and must be reachable by geometry.
    press(&mut app, KeyCode::End);
    assert_eq!(app.editor.as_ref().unwrap().selected, 59);
    let end = screen_text(&mut app);
    assert!(end.contains("&kp RIGHT_GUI"), "{end}");

    // All five requested layers are selectable by number.
    press(&mut app, KeyCode::Char('3'));
    assert_eq!(app.editor.as_ref().unwrap().layer_index, 3);
    let adjust = screen_text(&mut app);
    assert!(adjust.contains("Adjust"), "{adjust}");
    press(&mut app, KeyCode::Char('4'));
    assert_eq!(app.editor.as_ref().unwrap().layer_index, 4);
    let media = screen_text(&mut app);
    assert!(media.contains("Media"), "{media}");
    press(&mut app, KeyCode::BackTab);
    assert_eq!(app.editor.as_ref().unwrap().layer_index, 3);
}

#[tokio::test]
async fn adds_and_removes_layers() {
    let (mut app, mut rx) = connected_app().await;

    press(&mut app, KeyCode::Char('L'));
    assert!(screen_text(&mut app).contains("Layers"));
    press(&mut app, KeyCode::Char('a'));
    pump_until(&mut app, &mut rx, "the added layer", |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.keymap.layers.len() == 6 && app.busy.is_none())
            .unwrap_or(false)
    })
    .await;
    let editor = app.editor.as_ref().unwrap();
    assert_eq!(editor.layer_index, 5);
    assert_eq!(editor.keymap.layers[5].bindings.len(), 42);

    press(&mut app, KeyCode::Char('L'));
    press(&mut app, KeyCode::Char('x'));
    let confirm = screen_text(&mut app);
    assert!(confirm.contains("Remove layer"), "{confirm}");
    press(&mut app, KeyCode::Char('y'));
    pump_until(&mut app, &mut rx, "the removed layer", |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.keymap.layers.len() == 5 && app.busy.is_none())
            .unwrap_or(false)
    })
    .await;
    assert!(app.editor.as_ref().unwrap().layer_index < 5);
}

#[tokio::test]
async fn quit_warns_about_unsaved_changes() {
    let (mut app, mut rx) = connected_app().await;
    let a_value = keycodes::from_name("A").unwrap().value;

    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "key press");
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "a");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    pump_until(&mut app, &mut rx, "the binding update", |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.keymap.layers[0].bindings[0].param1 == a_value)
            .unwrap_or(false)
    })
    .await;

    press(&mut app, KeyCode::Char('q'));
    assert!(!app.should_quit);
    let confirm = screen_text(&mut app);
    assert!(confirm.contains("Unsaved changes"), "{confirm}");
    assert!(confirm.contains("Save & quit"), "{confirm}");

    press(&mut app, KeyCode::Char('d'));
    pump_until(&mut app, &mut rx, "quit after discard", |app| {
        app.should_quit
    })
    .await;
    assert!(app.should_quit);
}

#[tokio::test]
async fn renames_and_reorders_layers() {
    let (mut app, mut rx) = connected_app().await;

    press(&mut app, KeyCode::Char('L'));
    press(&mut app, KeyCode::Char('r'));
    for _ in 0.."Base".len() {
        press(&mut app, KeyCode::Backspace);
    }
    type_text(&mut app, "Root");
    press(&mut app, KeyCode::Enter);
    pump_until(&mut app, &mut rx, "the layer rename", |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.keymap.layers[0].name == "Root" && app.busy.is_none())
            .unwrap_or(false)
    })
    .await;

    press(&mut app, KeyCode::Char('L'));
    press(&mut app, KeyCode::Char('J'));
    pump_until(&mut app, &mut rx, "the layer move", |app| {
        app.editor
            .as_ref()
            .map(|editor| {
                editor.keymap.layers[1].name == "Root"
                    && editor.layer_index == 1
                    && app.busy.is_none()
            })
            .unwrap_or(false)
    })
    .await;
    let editor = app.editor.as_ref().unwrap();
    assert_eq!(editor.keymap.layers[0].name, "Lower");
}

#[tokio::test]
async fn restores_stock_settings() {
    let (mut app, mut rx) = connected_app().await;
    let a_value = keycodes::from_name("A").unwrap().value;

    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "key press");
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "a");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    pump_until(&mut app, &mut rx, "the binding update", |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.keymap.layers[0].bindings[0].param1 == a_value)
            .unwrap_or(false)
    })
    .await;

    press(&mut app, KeyCode::Char('s'));
    pump_until(&mut app, &mut rx, "the save", |app| {
        app.editor
            .as_ref()
            .map(|editor| !editor.dirty)
            .unwrap_or(false)
    })
    .await;

    press(&mut app, KeyCode::Char('R'));
    let confirm = screen_text(&mut app);
    assert!(confirm.contains("Restore stock settings"), "{confirm}");
    press(&mut app, KeyCode::Char('y'));
    pump_until(&mut app, &mut rx, "stock settings", |app| {
        app.editor
            .as_ref()
            .map(|editor| {
                editor.keymap.layers[0].bindings[0].param1
                    == keycodes::from_name("ESCAPE").unwrap().value
                    && !editor.dirty
                    && app.busy.is_none()
            })
            .unwrap_or(false)
    })
    .await;
}

#[tokio::test]
async fn switches_physical_layout() {
    let (mut app, mut rx) = connected_app().await;
    assert_eq!(app.editor.as_ref().unwrap().layouts.layouts.len(), 2);

    press(&mut app, KeyCode::Char('p'));
    let modal = screen_text(&mut app);
    assert!(modal.contains("Physical layout"), "{modal}");
    assert!(modal.contains("Corne wide"), "{modal}");

    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    pump_until(&mut app, &mut rx, "the layout switch", |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.layouts.active_index == 1 && app.busy.is_none())
            .unwrap_or(false)
    })
    .await;
    let editor = app.editor.as_ref().unwrap();
    assert!(editor.dirty);
    assert_eq!(editor.layouts.active().unwrap().name, "Corne wide");
}

#[tokio::test]
async fn renders_the_connect_screen_without_hardware() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = App::new(tx, StartOptions::default());
    let screen = screen_text(&mut app);
    for expected in [
        "ZMK Studio",
        "terminal keymap editor",
        "USB",
        "Bluetooth",
        "CONFIG_ZMK_STUDIO=y",
        "Port busy",
        "close ZMK Studio",
        "Enter connect",
    ] {
        assert!(
            screen.contains(expected),
            "missing {expected:?} in:\n{screen}"
        );
    }
}

#[tokio::test]
async fn navigates_by_geometry() {
    let (mut app, _rx) = connected_app().await;
    // Base layout: ESC at index 0, Q at index 1, TAB at index 12.
    press(&mut app, KeyCode::Right);
    assert_eq!(app.editor.as_ref().unwrap().selected, 1);
    press(&mut app, KeyCode::Down);
    assert_eq!(app.editor.as_ref().unwrap().selected, 13);
    press(&mut app, KeyCode::Up);
    assert_eq!(app.editor.as_ref().unwrap().selected, 1);
    press(&mut app, KeyCode::Home);
    assert_eq!(app.editor.as_ref().unwrap().selected, 0);

    press(&mut app, KeyCode::Tab);
    assert_eq!(app.editor.as_ref().unwrap().layer_index, 1);
    press(&mut app, KeyCode::Char('0'));
    assert_eq!(app.editor.as_ref().unwrap().layer_index, 0);
}

#[tokio::test]
async fn mouse_selects_and_double_clicks_a_key() {
    let (mut app, _rx) = connected_app().await;
    let _ = screen_text(&mut app);

    click_target(&mut app, MouseTarget::EditorKey(5));
    assert_eq!(app.editor.as_ref().unwrap().selected, 5);
    click_target(&mut app, MouseTarget::EditorKey(5));
    assert!(matches!(app.modal, Some(Modal::Binding(_))));
}

#[tokio::test]
async fn mouse_clicks_layer_tabs_and_editor_actions() {
    let (mut app, _rx) = connected_app().await;
    let _ = screen_text(&mut app);

    click_target(&mut app, MouseTarget::LayerTab(4));
    assert_eq!(app.editor.as_ref().unwrap().layer_index, 4);

    click_target(&mut app, MouseTarget::EditorQuit);
    // The simulator has no unsaved changes here, so quit is immediate.
    assert!(app.should_quit);
}

#[tokio::test]
async fn mouse_picks_a_behavior_and_keycode_then_applies() {
    let (mut app, mut rx) = connected_app().await;
    press(&mut app, KeyCode::Enter);
    let _ = screen_text(&mut app);

    let mod_tap = {
        let editor = app.editor.as_ref().unwrap();
        behavior_items(&editor.catalog, "")
            .iter()
            .position(|behavior| behavior.display_name == "Mod-Tap")
            .expect("Mod-Tap behavior")
    };

    // Wheel scrolling is handled through the same list state.
    let before = match &app.modal {
        Some(Modal::Binding(wizard)) => wizard.behavior_state.selected(),
        _ => None,
    };
    scroll_list(&mut app, ListKind::Behaviors, true);
    let after = match &app.modal {
        Some(Modal::Binding(wizard)) => wizard.behavior_state.selected(),
        _ => None,
    };
    assert_ne!(before, after);

    // Double-click the Mod-Tap row to choose it and enter Parameter 1.
    for _ in 0..2 {
        click_list(&mut app, ListKind::Behaviors, mod_tap);
    }
    assert!(matches!(
        &app.modal,
        Some(Modal::Binding(wizard)) if wizard.stage == Stage::Param1
    ));

    let _ = screen_text(&mut app);
    for _ in 0..2 {
        click_list(&mut app, ListKind::Params, 0);
    }
    assert!(matches!(
        &app.modal,
        Some(Modal::Binding(wizard)) if wizard.stage == Stage::Param2
    ));

    let _ = screen_text(&mut app);
    for _ in 0..2 {
        click_list(&mut app, ListKind::Params, 0);
    }
    assert!(matches!(
        &app.modal,
        Some(Modal::Binding(wizard)) if wizard.stage == Stage::Review
    ));

    let _ = screen_text(&mut app);
    click_target(&mut app, MouseTarget::BindingApply);
    pump_until(&mut app, &mut rx, "the mouse binding update", |app| {
        app.editor
            .as_ref()
            .map(|editor| {
                editor
                    .catalog
                    .name(editor.keymap.layers[0].bindings[0].behavior_id)
                    == Some("Mod-Tap")
                    && app.busy.is_none()
            })
            .unwrap_or(false)
    })
    .await;
}

#[tokio::test]
async fn mouse_clicks_save_and_confirm_choices() {
    let (mut app, mut rx) = connected_app().await;
    let a_value = keycodes::from_name("A").unwrap().value;

    // Create an unsaved edit via the keyboard, then save with the footer button.
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "key press");
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "a");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    pump_until(&mut app, &mut rx, "the binding update", |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.keymap.layers[0].bindings[0].param1 == a_value)
            .unwrap_or(false)
    })
    .await;

    let _ = screen_text(&mut app);
    click_target(&mut app, MouseTarget::EditorSave);
    pump_until(&mut app, &mut rx, "the mouse save", |app| {
        app.editor
            .as_ref()
            .map(|editor| !editor.dirty && app.busy.is_none())
            .unwrap_or(false)
    })
    .await;

    // Make it dirty again, ask to quit, and click "Discard & quit".
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "key press");
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "b");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    pump_until(&mut app, &mut rx, "the second binding update", |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.dirty && app.busy.is_none())
            .unwrap_or(false)
    })
    .await;

    press(&mut app, KeyCode::Char('q'));
    let _ = screen_text(&mut app);
    click_target(&mut app, MouseTarget::ConfirmChoice(1));
    pump_until(&mut app, &mut rx, "quit via mouse discard", |app| {
        app.should_quit
    })
    .await;
}

#[test]
fn connect_screen_navigates_and_warns_without_devices() {
    let mut app = disconnected_app();
    let screen = screen_text(&mut app);
    assert!(screen.contains("Connect"));

    press(&mut app, KeyCode::Tab);
    assert_eq!(app.connect.focus, Panel::Bluetooth);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Enter);
    assert!(
        app.status
            .as_ref()
            .unwrap()
            .text
            .contains("Bluetooth keyboard")
    );
    assert!(screen_text(&mut app).contains("Select a Bluetooth keyboard first"));

    press(&mut app, KeyCode::Tab);
    assert_eq!(app.connect.focus, Panel::Usb);
    press(&mut app, KeyCode::Enter);
    assert!(app.status.as_ref().unwrap().text.contains("USB keyboard"));

    press(&mut app, KeyCode::Char('q'));
    assert!(app.should_quit);
}

#[test]
fn mouse_focuses_connect_panels() {
    let mut app = disconnected_app();
    let _ = screen_text(&mut app);
    click_target(&mut app, MouseTarget::ConnectPanel(Panel::Bluetooth));
    assert_eq!(app.connect.focus, Panel::Bluetooth);
    click_target(&mut app, MouseTarget::ConnectPanel(Panel::Usb));
    assert_eq!(app.connect.focus, Panel::Usb);
}

#[tokio::test]
async fn mouse_selects_and_switches_a_layer_row() {
    let (mut app, _rx) = connected_app().await;
    press(&mut app, KeyCode::Char('L'));
    let _ = screen_text(&mut app);
    click_list(&mut app, ListKind::Layers, 1);
    assert!(matches!(
        &app.modal,
        Some(Modal::Layers(modal)) if modal.state.selected() == Some(1)
    ));
    click_list(&mut app, ListKind::Layers, 1);
    assert_eq!(app.editor.as_ref().unwrap().layer_index, 1);
    assert!(!matches!(app.modal, Some(Modal::Layers(_))));
}

#[tokio::test]
async fn ticks_advance_without_disturbing_the_editor() {
    let (mut app, _rx) = connected_app().await;
    let before = app.tick;
    app.handle(Msg::Tick);
    assert_eq!(app.tick, before + 1);
    assert!(app.editor.is_some());
}

#[tokio::test]
async fn device_events_update_lock_and_dirty_state() {
    let (mut app, _rx) = connected_app().await;
    let _ = screen_text(&mut app);

    app.handle(Msg::DeviceEvent(StudioEvent::UnsavedChangesChanged(true)));
    assert!(app.editor.as_ref().unwrap().dirty);

    app.handle(Msg::DeviceEvent(StudioEvent::LockStateChanged(
        LockState::Locked,
    )));
    assert!(app.editor.as_ref().unwrap().locked);
    assert!(matches!(app.modal, Some(Modal::Unlock(_))));

    app.handle(Msg::DeviceEvent(StudioEvent::LockStateChanged(
        LockState::Unlocked,
    )));
    assert!(!app.editor.as_ref().unwrap().locked);
    assert!(app.modal.is_none());
}

#[tokio::test]
async fn lock_poll_results_clear_or_set_lock_state() {
    let (mut app, _rx) = connected_app().await;

    app.handle(Msg::LockPolled(Ok(LockState::Locked)));
    assert!(app.editor.as_ref().unwrap().locked);

    app.modal = Some(Modal::Unlock(UnlockModal::default()));
    app.handle(Msg::LockPolled(Ok(LockState::Unlocked)));
    assert!(!app.editor.as_ref().unwrap().locked);
    assert!(app.modal.is_none());

    app.handle(Msg::LockPolled(Err(StudioError::Timeout)));
    assert_eq!(
        app.status.as_ref().map(|status| status.kind),
        Some(StatusKind::Error)
    );
}

#[test]
fn mouse_scroll_moves_connect_and_editor_selection() {
    let mut app = disconnected_app();
    app.connect.usb = vec![
        UsbDevice {
            path: "/dev/test0".into(),
            product: Some("Test Keyboard".into()),
            manufacturer: None,
            serial_number: None,
        },
        UsbDevice {
            path: "/dev/test1".into(),
            product: Some("Test Keyboard".into()),
            manufacturer: None,
            serial_number: None,
        },
    ];
    app.connect.usb_state.select(Some(0));
    let _ = screen_text(&mut app);
    scroll_list(&mut app, ListKind::UsbDevices, true);
    assert_eq!(app.connect.usb_state.selected(), Some(1));
    scroll_list(&mut app, ListKind::UsbDevices, false);
    assert_eq!(app.connect.usb_state.selected(), Some(0));
    assert_eq!(app.connect.focus, Panel::Usb);
}

#[tokio::test]
async fn mouse_wheel_navigates_the_keymap_without_a_list_under_it() {
    let (mut app, _rx) = connected_app().await;
    let _ = screen_text(&mut app);
    let target = app
        .mouse
        .target_rect(MouseTarget::EditorKey(5))
        .expect("key target");
    app.handle(Msg::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: target.x + 1,
        row: target.y + 1,
        modifiers: KeyModifiers::NONE,
    }));
    assert_ne!(app.editor.as_ref().unwrap().selected, 0);
}

#[tokio::test]
async fn editor_commands_open_and_close_modals_and_jump_layers() {
    let (mut app, _rx) = connected_app().await;

    press(&mut app, KeyCode::Char('L'));
    assert!(matches!(app.modal, Some(Modal::Layers(_))));
    press(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none());

    press(&mut app, KeyCode::Char('p'));
    assert!(matches!(app.modal, Some(Modal::Layouts(_))));
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char('u'));
    assert!(matches!(app.modal, Some(Modal::Unlock(_))));
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char('?'));
    assert!(matches!(app.modal, Some(Modal::Help)));
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char('R'));
    assert!(matches!(app.modal, Some(Modal::Confirm(_))));
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.modal, Some(Modal::Confirm(_))));
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char(']'));
    assert_eq!(app.editor.as_ref().unwrap().layer_index, 1);
    press(&mut app, KeyCode::Char('['));
    assert_eq!(app.editor.as_ref().unwrap().layer_index, 0);
    press(&mut app, KeyCode::Char('9'));
    assert_eq!(app.editor.as_ref().unwrap().layer_index, 0);
}

#[tokio::test]
async fn quit_confirmation_can_save_then_quit() {
    let (mut app, mut rx) = connected_app().await;
    app.editor.as_mut().unwrap().dirty = true;
    press(&mut app, KeyCode::Char('q'));
    assert!(matches!(app.modal, Some(Modal::Confirm(_))));
    press(&mut app, KeyCode::Char('s'));
    pump_until(&mut app, &mut rx, "save then quit", |app| app.should_quit).await;
}

#[tokio::test]
async fn quit_confirmation_can_quit_anyway() {
    let (mut app, _rx) = connected_app().await;
    app.editor.as_mut().unwrap().dirty = true;
    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Char('q'));
    assert!(app.should_quit);
}

#[tokio::test]
async fn unlock_modal_enter_starts_a_lock_poll() {
    let (mut app, mut rx) = connected_app().await;
    press(&mut app, KeyCode::Char('u'));
    assert!(matches!(&app.modal, Some(Modal::Unlock(unlock)) if !unlock.waiting));
    press(&mut app, KeyCode::Enter);
    assert!(matches!(&app.modal, Some(Modal::Unlock(unlock)) if unlock.waiting));
    pump_until(
        &mut app,
        &mut rx,
        "unlock poll",
        |app| !matches!(&app.modal, Some(Modal::Unlock(unlock)) if unlock.waiting),
    )
    .await;
}

#[test]
fn usb_device_messages_clamp_connect_selection() {
    let mut app = disconnected_app();
    app.connect.usb_state.select(Some(9));
    app.handle(Msg::UsbDevices(vec![UsbDevice {
        path: "/dev/test0".into(),
        product: Some("Test Keyboard".into()),
        manufacturer: None,
        serial_number: None,
    }]));
    assert_eq!(app.connect.usb_state.selected(), Some(0));

    app.handle(Msg::UsbDevices(Vec::new()));
    assert_eq!(app.connect.usb_state.selected(), None);
}

#[tokio::test]
async fn discard_resyncs_the_physical_layout() {
    let (mut app, mut rx) = connected_app().await;

    press(&mut app, KeyCode::Char('p'));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    pump_until(&mut app, &mut rx, "the layout switch", |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.layouts.active_index == 1 && app.busy.is_none())
            .unwrap_or(false)
    })
    .await;
    assert!(app.editor.as_ref().unwrap().dirty);

    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Char('y'));
    pump_until(&mut app, &mut rx, "the discard", |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.layouts.active_index == 0 && app.busy.is_none())
            .unwrap_or(false)
    })
    .await;
    assert!(!app.editor.as_ref().unwrap().dirty);
}

#[tokio::test]
async fn selecting_the_active_layout_is_not_an_edit() {
    let (mut app, mut rx) = connected_app().await;

    press(&mut app, KeyCode::Char('p'));
    // The picker starts on the active layout; Enter re-selects it.
    press(&mut app, KeyCode::Enter);
    pump_until(&mut app, &mut rx, "the no-op layout selection", |app| {
        app.busy.is_none() && app.modal.is_none()
    })
    .await;
    assert!(
        !app.editor.as_ref().unwrap().dirty,
        "re-selecting the current layout must not mark the keymap dirty"
    );
}

#[tokio::test]
async fn late_completions_do_not_close_other_modals() {
    let (mut app, _rx) = connected_app().await;
    let a = keycodes::from_name("A").unwrap().value;

    press(&mut app, KeyCode::Char('?'));
    assert!(matches!(app.modal, Some(Modal::Help)));

    app.handle(Msg::BindingSet {
        result: Ok(()),
        layer_id: 0,
        position: 0,
        binding: Binding {
            behavior_id: 1001,
            param1: a,
            param2: 0,
        },
    });
    assert!(
        matches!(app.modal, Some(Modal::Help)),
        "an in-flight binding update must not dismiss an unrelated dialog"
    );

    app.handle(Msg::LayoutChanged {
        result: Ok(app.editor.as_ref().unwrap().keymap.clone()),
        index: 1,
    });
    assert!(matches!(app.modal, Some(Modal::Help)));
}

#[tokio::test]
async fn layer_add_is_blocked_when_slots_are_full() {
    let (mut app, _rx) = connected_app().await;
    app.editor.as_mut().unwrap().keymap.available_layers = 0;

    press(&mut app, KeyCode::Char('L'));
    assert!(matches!(app.modal, Some(Modal::Layers(_))));
    press(&mut app, KeyCode::Char('a'));
    assert!(
        matches!(app.modal, Some(Modal::Layers(_))),
        "the manager stays open and explains why the layer cannot be added"
    );
    assert_eq!(
        app.status.as_ref().map(|status| status.kind),
        Some(StatusKind::Warning)
    );
    assert!(app.busy.is_none());
}

#[tokio::test]
async fn stale_snapshots_do_not_overwrite_newer_edits() {
    let (mut app, mut rx) = connected_app().await;
    let a_value = keycodes::from_name("A").unwrap().value;
    let before_edit = app.editor.as_ref().unwrap().keymap.clone();

    // Apply an edit through the wizard, which records a mutation.
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "key press");
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "a");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    pump_until(&mut app, &mut rx, "the binding update", |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.keymap.layers[0].bindings[0].param1 == a_value)
            .unwrap_or(false)
    })
    .await;

    let editor = app.editor.as_ref().unwrap();
    let mut layouts = editor.layouts.clone();
    layouts.active_index = 1;
    let snapshot = Snapshot {
        keymap: before_edit,
        layouts,
        catalog: editor.catalog.clone(),
        dirty: false,
        warnings: vec![],
    };
    app.handle(Msg::Snapshot(Ok(snapshot)));

    let editor = app.editor.as_ref().unwrap();
    assert_eq!(
        editor.keymap.layers[0].bindings[0].param1, a_value,
        "the stale snapshot must not revert the newer edit"
    );
    assert!(editor.dirty, "the newer edit is still unsaved");
    assert_eq!(
        editor.layouts.active_index, 1,
        "layout data from the snapshot is still applied"
    );
}

#[tokio::test]
async fn failed_keymap_load_is_retried() {
    let (mut app, mut rx) = connected_app().await;
    app.editor.as_mut().unwrap().keymap_loaded = false;

    app.handle(Msg::Snapshot(Err(StudioError::Timeout)));
    assert!(app.busy.is_none());

    tokio::time::sleep(Duration::from_millis(1100)).await;
    app.handle(Msg::Tick);
    assert!(
        app.busy.is_some(),
        "a failed load must be retried automatically"
    );

    pump_until(&mut app, &mut rx, "the retried keymap load", |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.keymap_loaded && app.busy.is_none())
            .unwrap_or(false)
    })
    .await;
}

/// Every visible key rectangle must hit its own key, unless it overlaps another
/// key rectangle (overlapping projections are legitimate on rotated boards).
fn assert_editor_key_targets_are_consistent(app: &mut App) {
    let count = app.editor.as_ref().unwrap().position_count();
    let rects: Vec<(usize, ratatui::layout::Rect)> = (0..count)
        .filter_map(|position| {
            app.mouse
                .target_rect(MouseTarget::EditorKey(position))
                .map(|rect| (position, rect))
        })
        .collect();
    for (position, rect) in &rects {
        let overlaps = rects.iter().any(|(other, candidate)| {
            other != position
                && candidate.x < rect.x + rect.width
                && candidate.x + candidate.width > rect.x
                && candidate.y < rect.y + rect.height
                && candidate.y + candidate.height > rect.y
        });
        if overlaps {
            continue;
        }
        let target = app
            .mouse
            .hit(rect.x + rect.width / 2, rect.y + rect.height / 2);
        assert_eq!(
            target,
            Some(MouseTarget::EditorKey(*position)),
            "key {position} at {rect:?} is covered by another target"
        );
    }
}

#[tokio::test]
async fn scrolled_keymap_clips_offscreen_keys() {
    let (mut app, _rx) = connected_app_for(SimulatedBoard::Sofle).await;
    screen_text_sized(&mut app, 46, 12);

    // Scroll to the last thumb key, which forces the grid to scroll down.
    press(&mut app, KeyCode::End);
    screen_text_sized(&mut app, 46, 12);
    assert_editor_key_targets_are_consistent(&mut app);

    // The top row is scrolled out of view, so its keys must be clipped rather
    // than clamped onto the top visible row (where they would be drawn over and
    // steal clicks from the keys that actually belong there).
    assert!(
        app.mouse.target_rect(MouseTarget::EditorKey(2)).is_none(),
        "a fully scrolled-out key must not be rendered"
    );
}

#[tokio::test]
async fn layer_tabs_keep_the_active_layer_clickable_when_narrow() {
    let (mut app, _rx) = connected_app().await;
    // Add enough layers to overflow the tab bar at the minimum terminal width.
    let base = app.editor.as_ref().unwrap().keymap.layers[0].clone();
    {
        let editor = app.editor.as_mut().unwrap();
        for index in 0..3 {
            let mut layer = base.clone();
            layer.id = 100 + index;
            layer.name = format!("Extra{index}");
            editor.keymap.layers.push(layer);
        }
        editor.layer_index = editor.keymap.layers.len() - 1;
    }
    screen_text_sized(&mut app, 46, 12);

    let active = 7;
    let rect = app
        .mouse
        .target_rect(MouseTarget::LayerTab(active))
        .unwrap_or_else(|| panic!("active layer tab {active} must be visible"));
    assert!(rect.width > 0);
    assert_eq!(
        app.mouse.hit(rect.x + rect.width / 2, rect.y),
        Some(MouseTarget::LayerTab(active))
    );
}

#[tokio::test]
async fn confirm_choices_stay_clickable_on_a_narrow_terminal() {
    let (mut app, _rx) = connected_app().await;
    app.editor.as_mut().unwrap().dirty = true;
    press(&mut app, KeyCode::Char('q'));
    assert!(matches!(app.modal, Some(Modal::Confirm(_))));
    screen_text_sized(&mut app, 46, 12);

    for index in 0..3 {
        let rect = app
            .mouse
            .target_rect(MouseTarget::ConfirmChoice(index))
            .unwrap_or_else(|| panic!("choice {index} must be clickable"));
        assert_eq!(
            app.mouse.hit(rect.x + rect.width / 2, rect.y),
            Some(MouseTarget::ConfirmChoice(index))
        );
    }
}

#[tokio::test]
async fn binding_review_apply_target_covers_only_the_button() {
    let (mut app, _rx) = connected_app().await;
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "transparent");
    press(&mut app, KeyCode::Enter);
    screen_text(&mut app);
    let rect = app
        .mouse
        .target_rect(MouseTarget::BindingApply)
        .expect("apply target");
    assert!(
        rect.width <= 10,
        "only the [ Apply ] label should be clickable, got {rect:?}"
    );
}

#[tokio::test]
async fn vertical_navigation_follows_rendered_rows() {
    let (mut app, _rx) = connected_app_for(SimulatedBoard::Sofle).await;
    screen_text(&mut app);

    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right);
    assert_eq!(app.editor.as_ref().unwrap().selected, 2);

    // Sofle staggers the first column down by 0.37u; position 0 is nonetheless
    // on the same rendered row as position 2, so Up must not move there.
    press(&mut app, KeyCode::Up);
    assert_eq!(app.editor.as_ref().unwrap().selected, 2);

    press(&mut app, KeyCode::Home);
    press(&mut app, KeyCode::Down);
    assert_eq!(
        app.editor.as_ref().unwrap().selected,
        12,
        "Down keeps the same column on the next row"
    );
}

#[tokio::test]
async fn trackball_layouts_render_and_navigate() {
    for (board, name, count) in [
        (
            SimulatedBoard::Lily58,
            "Lily58 + trackball (simulator)",
            58usize,
        ),
        (
            SimulatedBoard::Split46,
            "Split 46 + trackball (simulator)",
            46usize,
        ),
    ] {
        let (mut app, _rx) = connected_app_for(board).await;
        let editor = app.editor.as_ref().expect("editor");
        assert_eq!(editor.info.name, name);
        assert_eq!(editor.keymap.layers.len(), 5);
        assert_eq!(editor.position_count(), count);
        assert_eq!(editor.layouts.layouts.len(), 1);
        assert_eq!(editor.layouts.layouts[0].keys.len(), count);

        let screen = screen_text(&mut app);
        assert!(screen.contains(name), "{screen}");
        for layer in ["Base", "Lower", "Raise", "Adjust", "Media"] {
            assert!(
                screen.contains(layer),
                "missing layer {layer} in:\n{screen}"
            );
        }

        press(&mut app, KeyCode::End);
        assert_eq!(app.editor.as_ref().unwrap().selected, count - 1);
        press(&mut app, KeyCode::Home);
        assert_eq!(app.editor.as_ref().unwrap().selected, 0);
        press(&mut app, KeyCode::Down);
        assert_eq!(
            app.editor.as_ref().unwrap().selected,
            12,
            "Down keeps the same column on the next row"
        );

        // The trackball's click buttons (if wired as matrix keys) are ordinary
        // bindings; the last thumb position is editable.
        assert_editor_key_targets_are_consistent(&mut app);
    }
}
