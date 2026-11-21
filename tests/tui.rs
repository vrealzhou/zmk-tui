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
use zmk_tui::model::LockState;
use zmk_tui::simulator::SimulatedBoard;
use zmk_tui::transport::serial::UsbDevice;
use zmk_tui::tui::StartOptions;
use zmk_tui::tui::app::{App, Msg, Panel, StatusKind};
use zmk_tui::tui::modals::{Modal, Stage, UnlockModal, behavior_items};
use zmk_tui::tui::mouse::{ListKind, MouseTarget};
use zmk_tui::tui::render;

fn screen_text(app: &mut App) -> String {
    let backend = TestBackend::new(120, 35);
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
