//! Executable Gherkin specifications for the editor's user-visible workflows.

use std::fmt;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cucumber::{World as _, given, then, when};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tokio::sync::mpsc::{self, UnboundedReceiver};

use zmk_tui::simulator::SimulatedBoard;
use zmk_tui::tui::StartOptions;
use zmk_tui::tui::app::{App, Msg};
use zmk_tui::tui::modals::{Modal, Stage};
use zmk_tui::tui::render;

#[derive(cucumber::World, Default)]
struct KeymapWorld {
    app: Option<App>,
    rx: Option<UnboundedReceiver<Msg>>,
}

impl fmt::Debug for KeymapWorld {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeymapWorld")
            .field("has_app", &self.app.is_some())
            .finish()
    }
}

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
    for character in text.chars() {
        press(app, KeyCode::Char(character));
    }
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

impl KeymapWorld {
    fn app(&mut self) -> &mut App {
        self.app.as_mut().expect("a keyboard is connected")
    }
}

#[given(expr = "a simulated {word} keyboard is connected")]
async fn connect_simulated(world: &mut KeymapWorld, board: String) {
    let board = match board.to_lowercase().as_str() {
        "corne" => SimulatedBoard::Corne,
        "sofle" => SimulatedBoard::Sofle,
        other => panic!("unknown simulated board {other}"),
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = App::new(
        tx,
        StartOptions {
            simulate: Some(board),
            ..Default::default()
        },
    );
    app.on_start();
    pump_until(&mut app, &mut rx, "the simulated keymap", |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.keymap_loaded)
            .unwrap_or(false)
    })
    .await;
    world.app = Some(app);
    world.rx = Some(rx);
}

#[then(expr = "the editor shows {string}")]
async fn editor_shows(world: &mut KeymapWorld, expected: String) {
    let screen = screen_text(world.app());
    assert!(
        screen.contains(&expected),
        "missing {expected:?} in:\n{screen}"
    );
}

#[when("I open the binding editor")]
async fn open_binding_editor(world: &mut KeymapWorld) {
    press(world.app(), KeyCode::Enter);
    assert!(
        matches!(world.app().modal, Some(Modal::Binding(_))),
        "binding editor did not open"
    );
}

#[when(expr = "I filter the behaviors by {string}")]
async fn filter_behaviors(world: &mut KeymapWorld, filter: String) {
    type_text(world.app(), &filter);
}

#[when("I choose the first behavior")]
async fn choose_first_behavior(world: &mut KeymapWorld) {
    press(world.app(), KeyCode::Enter);
    assert!(
        matches!(
            &world.app().modal,
            Some(Modal::Binding(wizard)) if wizard.stage == Stage::Param1
        ),
        "expected Parameter 1 after choosing the behavior"
    );
}

#[when(expr = "I filter the keycodes by {string}")]
async fn filter_keycodes(world: &mut KeymapWorld, filter: String) {
    type_text(world.app(), &filter);
}

#[when("I choose the first keycode")]
async fn choose_first_keycode(world: &mut KeymapWorld) {
    press(world.app(), KeyCode::Enter);
}

#[then("the binding review is shown")]
async fn review_is_shown(world: &mut KeymapWorld) {
    assert!(
        matches!(
            &world.app().modal,
            Some(Modal::Binding(wizard)) if wizard.stage == Stage::Review
        ),
        "expected the binding review"
    );
    assert!(screen_text(world.app()).contains("Review"));
}

#[when("I apply the binding")]
async fn apply_binding(world: &mut KeymapWorld) {
    press(world.app(), KeyCode::Enter);
    let mut app = world.app.take().expect("app");
    let mut rx = world.rx.take().expect("receiver");
    pump_until(&mut app, &mut rx, "the binding update", |app| {
        app.modal.is_none() && app.busy.is_none()
    })
    .await;
    world.app = Some(app);
    world.rx = Some(rx);
}

#[then("the keymap is dirty")]
async fn keymap_is_dirty(world: &mut KeymapWorld) {
    assert!(world.app().editor.as_ref().unwrap().dirty);
}

#[when("I save the keymap")]
async fn save_keymap(world: &mut KeymapWorld) {
    press(world.app(), KeyCode::Char('s'));
    let mut app = world.app.take().expect("app");
    let mut rx = world.rx.take().expect("receiver");
    pump_until(&mut app, &mut rx, "the save", |app| {
        app.editor
            .as_ref()
            .map(|editor| !editor.dirty && app.busy.is_none())
            .unwrap_or(false)
    })
    .await;
    world.app = Some(app);
    world.rx = Some(rx);
}

#[then("the keymap is clean")]
async fn keymap_is_clean(world: &mut KeymapWorld) {
    let editor = world.app().editor.as_ref().unwrap();
    assert!(!editor.dirty);
}

#[when("I request to quit")]
async fn request_quit(world: &mut KeymapWorld) {
    press(world.app(), KeyCode::Char('q'));
}

#[then("the quit confirmation is shown")]
async fn quit_confirmation(world: &mut KeymapWorld) {
    assert!(
        matches!(world.app().modal, Some(Modal::Confirm(_))),
        "quit confirmation did not open"
    );
}

#[when("I discard the changes and quit")]
async fn discard_and_quit(world: &mut KeymapWorld) {
    press(world.app(), KeyCode::Char('d'));
    let mut app = world.app.take().expect("app");
    let mut rx = world.rx.take().expect("receiver");
    pump_until(&mut app, &mut rx, "quit", |app| app.should_quit).await;
    world.app = Some(app);
    world.rx = Some(rx);
}

#[then("the application should quit")]
async fn application_quit(world: &mut KeymapWorld) {
    assert!(world.app().should_quit);
}

#[when(expr = "I switch to layer {int}")]
async fn switch_layer(world: &mut KeymapWorld, layer: usize) {
    press(
        world.app(),
        KeyCode::Char(char::from_digit(layer as u32, 10).unwrap()),
    );
}

#[tokio::main]
async fn main() {
    KeymapWorld::cucumber()
        .fail_on_skipped()
        .run_and_exit("tests/features")
        .await;
}
