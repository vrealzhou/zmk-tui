//! Render the UI to JSON cell dumps for documentation screenshots.
//!
//! Usage:
//!   cargo run --example screens -- /tmp/zmk-shots
//!
//! Then `python3 tools/render_screenshot.py /tmp/zmk-shots docs`.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use serde_json::{Value, json};
use tokio::sync::mpsc::{self, UnboundedReceiver};

use zmk_tui::simulator::SimulatedBoard;
use zmk_tui::tui::StartOptions;
use zmk_tui::tui::app::{App, Msg};
use zmk_tui::tui::render;

const WIDTH: u16 = 120;
const HEIGHT: u16 = 36;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/zmk-shots".into());
    std::fs::create_dir_all(&output)?;

    capture(SimulatedBoard::Corne, "", Path::new(&output)).await?;
    capture(SimulatedBoard::Sofle, "sofle-", Path::new(&output)).await?;

    println!("wrote screens to {output}");
    Ok(())
}

async fn capture(board: SimulatedBoard, prefix: &str, output: &Path) -> anyhow::Result<()> {
    let (tx, mut messages) = mpsc::unbounded_channel();
    let mut app = App::new(
        tx,
        StartOptions {
            simulate: Some(board),
            ..Default::default()
        },
    );
    app.on_start();
    pump_until(&mut app, &mut messages, |app| {
        app.editor
            .as_ref()
            .map(|editor| editor.keymap_loaded)
            .unwrap_or(false)
    })
    .await;

    let path = |name: &str| output.join(format!("{prefix}{name}.json"));
    dump(&mut app, &path("editor"))?;

    press(&mut app, KeyCode::Enter);
    dump(&mut app, &path("binding-behavior"))?;

    // Filter to Mod-Tap, then open its parameter list.
    for ch in "mod".chars() {
        press(&mut app, KeyCode::Char(ch));
    }
    dump(&mut app, &path("binding-filtered"))?;
    press(&mut app, KeyCode::Enter);
    dump(&mut app, &path("binding-param"))?;

    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('L'));
    dump(&mut app, &path("layers"))?;
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char('?'));
    dump(&mut app, &path("help"))?;
    Ok(())
}

fn press(app: &mut App, code: KeyCode) {
    app.handle(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

async fn pump_until(
    app: &mut App,
    messages: &mut UnboundedReceiver<Msg>,
    predicate: impl Fn(&App) -> bool,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if predicate(app) {
            return;
        }
        let message = tokio::time::timeout_at(deadline, messages.recv())
            .await
            .expect("timed out connecting to the simulator")
            .expect("message channel closed");
        app.handle(message);
    }
}

fn dump(app: &mut App, path: &Path) -> anyhow::Result<()> {
    let backend = TestBackend::new(WIDTH, HEIGHT);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|frame| render::render(frame, app))?;
    let buffer = terminal.backend().buffer();

    let mut rows: Vec<Value> = Vec::new();
    for y in 0..buffer.area.height {
        let mut row: Vec<Value> = Vec::new();
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            row.push(json!({
                "ch": cell.symbol(),
                "fg": color(cell.fg),
                "bg": color(cell.bg),
                "bold": cell.modifier.contains(Modifier::BOLD),
                "dim": cell.modifier.contains(Modifier::DIM),
            }));
        }
        rows.push(Value::Array(row));
    }

    let document = json!({
        "width": buffer.area.width,
        "height": buffer.area.height,
        "rows": rows,
    });
    std::fs::write(path, serde_json::to_string(&document)?)
        .with_context(|| format!("could not write {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

fn color(color: Color) -> Value {
    match color {
        Color::Rgb(r, g, b) => json!([r, g, b]),
        Color::Reset => Value::Null,
        Color::Black => json!([0, 0, 0]),
        Color::White => json!([255, 255, 255]),
        Color::Indexed(index) => json!(["indexed", index]),
        other => json!([format!("{other:?}")]),
    }
}
