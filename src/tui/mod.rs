//! Terminal UI entry point and event loop.

pub mod app;
pub mod connection;
pub mod keyboard;
pub mod modals;
pub mod mouse;
pub mod render;
pub mod text;
pub mod theme;

pub use app::StartOptions;

use std::io::IsTerminal;
use std::time::Duration;

use anyhow::{Context, bail};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseEventKind,
};
use tokio::sync::mpsc;

use app::{App, Msg};

/// Run the interactive editor. Must be called from a Tokio runtime.
pub async fn run(start: StartOptions) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("zmk-tui needs an interactive terminal: stdin is not a TTY");
    }
    if !std::io::stdout().is_terminal() {
        bail!("zmk-tui needs an interactive terminal: stdout is not a TTY");
    }

    let mut terminal = ratatui::init();
    if let Err(error) = crossterm::execute!(std::io::stdout(), EnableMouseCapture) {
        ratatui::restore();
        return Err(error.into());
    }

    // Keep the terminal usable if we panic: disable mouse reporting before
    // running ratatui's own restore hook.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        previous_hook(info);
    }));

    let result = run_loop(&mut terminal, start).await;
    // `run_loop` drops the app (and its BLE sink) on exit; give the graceful
    // disconnect a moment to run before the runtime is torn down.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

async fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    start: StartOptions,
) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    spawn_input_thread(tx.clone());

    let mut app = App::new(tx, start);
    app.on_start();

    let mut ticker = tokio::time::interval(Duration::from_millis(150));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        terminal
            .draw(|frame| render::render(frame, &mut app))
            .context("could not draw the terminal UI")?;

        if app.should_quit {
            break;
        }

        tokio::select! {
            message = rx.recv() => {
                match message {
                    Some(message) => app.handle(message),
                    None => bail!("terminal input ended unexpectedly"),
                }
            }
            _ = ticker.tick() => app.handle(Msg::Tick),
        }
    }

    Ok(())
}

fn spawn_input_thread(tx: mpsc::UnboundedSender<Msg>) {
    std::thread::Builder::new()
        .name("zmk-tui-input".into())
        .spawn(move || {
            loop {
                match crossterm::event::read() {
                    Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                        if tx.send(Msg::Key(key)).is_err() {
                            break;
                        }
                    }
                    Ok(Event::Mouse(mouse))
                        if matches!(
                            mouse.kind,
                            MouseEventKind::Down(_)
                                | MouseEventKind::ScrollUp
                                | MouseEventKind::ScrollDown
                        ) =>
                    {
                        if tx.send(Msg::Mouse(mouse)).is_err() {
                            break;
                        }
                    }
                    Ok(Event::Resize(_, _)) => {
                        if tx.send(Msg::Tick).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        })
        .expect("spawn input thread");
}
