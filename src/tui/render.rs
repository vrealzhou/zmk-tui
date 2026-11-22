//! Rendering for all screens and modal dialogs.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::keycodes;
use crate::labels;
use crate::tui::app::{App, EditorState, Panel, Status, StatusKind};
use crate::tui::keyboard;
use crate::tui::modals::{
    BindingWizard, ConfirmModal, LayerModal, LayoutModal, Modal, Stage, behavior_items, param_desc,
    param_items, param_set,
};
use crate::tui::mouse::{ListHit, ListKind, MouseMap, MouseTarget};
use crate::tui::{text, theme};

const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

fn spinner(tick: u64) -> &'static str {
    SPINNER[(tick as usize / 2) % SPINNER.len()]
}

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(theme::base()), area);

    if area.width < 46 || area.height < 12 {
        app.mouse = MouseMap::default();
        render_too_small(frame, area);
        return;
    }

    let mut hits = MouseMap::default();
    if app.editor.is_some() {
        render_editor(frame, area, app, &mut hits);
    } else {
        render_connect(frame, area, app, &mut hits);
    }

    if app.modal.is_some() {
        // Only the modal is interactive while it is open.
        hits = MouseMap::default();
        render_modal(frame, area, app, &mut hits);
    }
    app.mouse = hits;
}

fn render_too_small(frame: &mut Frame, area: Rect) {
    let paragraph = Paragraph::new(vec![
        Line::from(Span::styled("Terminal too small", theme::title())),
        Line::from(Span::styled(
            "Resize to at least 46 × 12 to use the keymap editor.",
            theme::dim(),
        )),
    ])
    .alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}

// --------------------------------------------------------------------------
// Editor
// --------------------------------------------------------------------------

fn render_editor(frame: &mut Frame, area: Rect, app: &mut App, hits: &mut MouseMap) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Length(1), // layers
        Constraint::Min(5),    // keyboard
        Constraint::Length(2), // selection detail
        Constraint::Length(1), // footer
    ])
    .split(area);
    // Remember the keyboard viewport so directional navigation can use the same
    // projection the keymap is drawn with.
    app.keyboard_viewport = crate::layout::Viewport {
        width: chunks[2].width.saturating_sub(2),
        height: chunks[2].height.saturating_sub(2),
    };

    let App {
        editor,
        status,
        busy,
        tick,
        ..
    } = app;
    let Some(editor) = editor.as_ref() else {
        return;
    };

    render_editor_header(frame, chunks[0], editor);
    render_layer_tabs(frame, chunks[1], editor, hits);
    render_editor_body(frame, chunks[2], editor, *tick, busy.as_deref(), hits);
    render_detail(frame, chunks[3], editor);
    render_editor_footer(
        frame,
        chunks[4],
        status.as_ref(),
        busy.as_deref(),
        *tick,
        if editor.locked {
            "u unlock · ↑↓←→ move · Tab layer · L layers · b devices · ? help"
        } else {
            "↑↓←→ move · Enter edit · Tab layer · L layers · b devices · ? help"
        },
        hits,
    );
}

fn render_editor_header(frame: &mut Frame, area: Rect, editor: &EditorState) {
    let parts = Layout::horizontal([Constraint::Min(10), Constraint::Length(34)]).split(area);
    let name_width = parts[0].width.saturating_sub(12) as usize;
    let name = text::ellipsize(&editor.info.name, name_width.max(4));
    let mut left_spans = vec![
        Span::styled(" ZMK ", theme::title()),
        Span::styled(name, Style::default().fg(theme::TEXT)),
        Span::styled("  ", Style::default().fg(theme::TEXT_DIM)),
    ];
    if editor.label != editor.info.name && !editor.label.is_empty() {
        left_spans.push(Span::styled(
            text::ellipsize(&editor.label, 28),
            Style::default().fg(theme::TEXT_DIM),
        ));
    }
    let left = Line::from(left_spans);
    frame.render_widget(Paragraph::new(left), parts[0]);

    let mut badges = Vec::new();
    if editor.locked {
        badges.push(Span::styled(
            " LOCKED ",
            Style::default()
                .bg(theme::DANGER)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ));
        badges.push(Span::raw(" "));
    }
    if editor.dirty {
        badges.push(Span::styled(
            " ● UNSAVED ",
            Style::default().fg(theme::WARNING),
        ));
        badges.push(Span::raw(" "));
    }
    badges.push(Span::styled(
        format!(" {} ", editor.kind.label().to_uppercase()),
        Style::default()
            .bg(theme::PANEL_ALT)
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(
        Paragraph::new(Line::from(badges)).alignment(Alignment::Right),
        parts[1],
    );
}

fn render_layer_tabs(frame: &mut Frame, area: Rect, editor: &EditorState, hits: &mut MouseMap) {
    let layers = &editor.keymap.layers;
    if layers.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" No layers loaded", theme::dim()))),
            area,
        );
        return;
    }

    let active = editor.layer_index.min(layers.len() - 1);
    let prefix = " Layers ";
    let prefix_width = text::width(prefix) as u16;
    let labels: Vec<String> = layers
        .iter()
        .enumerate()
        .map(|(index, layer)| format!(" {index} {} ", text::ellipsize(&layer.label(index), 12)))
        .collect();
    let widths: Vec<u16> = labels
        .iter()
        .map(|label| text::width(label) as u16 + 1)
        .collect();

    // Fit as many tabs as the terminal allows, keeping the active layer
    // visible. Two columns are reserved for each scroll indicator.
    let budget = area.width.saturating_sub(prefix_width.saturating_add(4));
    let mut start = active;
    let mut used = widths[active].min(budget);
    while start > 0 && used.saturating_add(widths[start - 1]) <= budget {
        start -= 1;
        used = used.saturating_add(widths[start]);
    }
    let mut end = active + 1;
    while end < layers.len() && used.saturating_add(widths[end]) <= budget {
        used = used.saturating_add(widths[end]);
        end += 1;
    }

    let mut spans = vec![Span::styled(prefix, theme::title())];
    let mut x = area.x.saturating_add(prefix_width);
    if start > 0 {
        spans.push(Span::styled("‹ ", theme::dim()));
        x = x.saturating_add(2);
    }
    for (index, label) in labels.iter().enumerate().take(end).skip(start) {
        let width = text::width(label) as u16;
        let remaining = area.right().saturating_sub(x);
        if remaining > 0 {
            hits.add(
                Rect {
                    x,
                    y: area.y,
                    width: width.min(remaining),
                    height: 1,
                },
                MouseTarget::LayerTab(index),
            );
        }
        if index == active {
            spans.push(Span::styled(label.clone(), theme::selected()));
        } else {
            spans.push(Span::styled(
                label.clone(),
                Style::default().fg(theme::TEXT_DIM).bg(theme::PANEL),
            ));
        }
        spans.push(Span::raw(" "));
        x = x.saturating_add(width).saturating_add(1);
    }
    if end < layers.len() {
        spans.push(Span::styled("›", theme::dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_editor_body(
    frame: &mut Frame,
    area: Rect,
    editor: &EditorState,
    tick: u64,
    busy: Option<&str>,
    hits: &mut MouseMap,
) {
    if editor.keymap_loaded {
        keyboard::render(frame, area, editor, hits);
        return;
    }

    let lines = if editor.locked {
        vec![
            Line::from(Span::styled("🔒", Style::default().fg(theme::DANGER))),
            Line::from(""),
            Line::from(Span::styled("Keyboard is locked", theme::title())),
            Line::from(Span::styled(
                "Press the key or combo bound to &studio_unlock.",
                Style::default().fg(theme::TEXT),
            )),
            Line::from(Span::styled(
                "The app will load the keymap automatically.",
                theme::dim(),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Press u for unlock instructions",
                theme::dim(),
            )),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                format!("{} Reading keymap…", spinner(tick)),
                Style::default().fg(theme::ACCENT),
            )),
            Line::from(""),
            Line::from(Span::styled(
                busy.unwrap_or("Waiting for the keyboard"),
                theme::dim(),
            )),
        ]
    };
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}

fn render_detail(frame: &mut Frame, area: Rect, editor: &EditorState) {
    let binding = editor.selected_binding().unwrap_or_default();
    let label = labels::binding_label(&editor.catalog, &editor.keymap, binding);
    let behavior_name = editor
        .catalog
        .name(binding.behavior_id)
        .unwrap_or("unknown behavior");
    let layer_name = editor.layer_name();

    let head = vec![
        Span::styled(" › ", Style::default().fg(theme::ACCENT)),
        Span::styled(
            format!("pos {:<2}", editor.selected),
            Style::default().fg(theme::TEXT_DIM),
        ),
        Span::styled("  ·  ", theme::dim()),
        Span::styled(layer_name, Style::default().fg(theme::TEXT)),
        Span::styled("  ·  ", theme::dim()),
        Span::styled(
            text::ellipsize(editor.active_layout_name(), 20),
            Style::default().fg(theme::ACCENT_ALT),
        ),
        Span::styled("  ·  ", theme::dim()),
    ];
    let head_width: usize = head
        .iter()
        .map(|span| text::width(span.content.as_ref()))
        .sum();
    let mut first_spans = head;
    first_spans.push(Span::styled(
        text::ellipsize(
            &label.detail,
            (area.width as usize).saturating_sub(head_width),
        ),
        Style::default().fg(theme::TEXT),
    ));
    let first = Line::from(first_spans);

    let second = if editor.locked {
        Line::from(Span::styled(
            "     LOCKED — press u and your Studio Unlock combo to edit",
            Style::default().fg(theme::DANGER),
        ))
    } else if binding.is_empty() {
        Line::from(Span::styled(
            "     unassigned — press Enter to choose a behavior",
            theme::dim(),
        ))
    } else {
        let extra = match binding.behavior_id {
            _ if behavior_name == "Key Press" => {
                format!("keycode: {}", keycodes::describe(binding.param1))
            }
            _ => format!("behavior: {behavior_name}"),
        };
        let header = format!("     {} ", text::fit_padded(&label.header, 6));
        let header_width = text::width(&header);
        let main_span = Span::styled(label.main.clone(), Style::default().fg(theme::TEXT));
        let used = header_width + text::width(&label.main) + 3;
        Line::from(vec![
            Span::styled(header, Style::default().fg(theme::ACCENT_ALT)),
            main_span,
            Span::styled("   ", theme::dim()),
            Span::styled(
                text::ellipsize(&extra, (area.width as usize).saturating_sub(used)),
                theme::dim(),
            ),
        ])
    };

    frame.render_widget(Paragraph::new(vec![first, second]), area);
}

fn footer_line(
    status: Option<&Status>,
    busy: Option<&str>,
    tick: u64,
    hints: &'static str,
    width: usize,
) -> Line<'static> {
    if let Some(busy) = busy {
        Line::from(vec![
            Span::styled(
                format!(" {} ", spinner(tick)),
                Style::default().fg(theme::WARNING),
            ),
            Span::styled(busy.to_string(), Style::default().fg(theme::WARNING)),
        ])
    } else if let Some(status) = status {
        let color = match status.kind {
            StatusKind::Info => theme::ACCENT,
            StatusKind::Success => theme::SUCCESS,
            StatusKind::Warning => theme::WARNING,
            StatusKind::Error => theme::DANGER,
        };
        Line::from(vec![
            Span::styled(" ● ", Style::default().fg(color)),
            Span::styled(
                text::ellipsize(&status.text, width.saturating_sub(4)),
                Style::default().fg(color),
            ),
        ])
    } else {
        Line::from(Span::styled(hints, theme::dim()))
    }
}

fn render_footer(
    frame: &mut Frame,
    area: Rect,
    status: Option<&Status>,
    busy: Option<&str>,
    tick: u64,
    hints: &'static str,
) {
    frame.render_widget(
        Paragraph::new(footer_line(status, busy, tick, hints, area.width as usize)),
        area,
    );
}

fn render_editor_footer(
    frame: &mut Frame,
    area: Rect,
    status: Option<&Status>,
    busy: Option<&str>,
    tick: u64,
    hints: &'static str,
    hits: &mut MouseMap,
) {
    let parts = Layout::horizontal([Constraint::Min(12), Constraint::Length(34)]).split(area);
    frame.render_widget(
        Paragraph::new(footer_line(
            status,
            busy,
            tick,
            hints,
            parts[0].width as usize,
        )),
        parts[0],
    );

    let mut spans = Vec::new();
    let mut x = parts[1].x;
    for (key, label, target) in [
        ("S", "Save", MouseTarget::EditorSave),
        ("D", "Discard", MouseTarget::EditorDiscard),
        ("Q", "Quit", MouseTarget::EditorQuit),
    ] {
        let label = format!("[{key}] {label}");
        let width = text::width(&label) as u16;
        let remaining = parts[1].right().saturating_sub(x);
        if remaining > 0 {
            hits.add(
                Rect {
                    x,
                    y: area.y,
                    width: width.min(remaining),
                    height: 1,
                },
                target,
            );
        }
        spans.push(Span::styled(
            label,
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        x = x.saturating_add(width).saturating_add(1);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), parts[1]);
}

// --------------------------------------------------------------------------
// Connect screen
// --------------------------------------------------------------------------

fn render_connect(frame: &mut Frame, area: Rect, app: &mut App, hits: &mut MouseMap) {
    let App {
        connect,
        status,
        busy,
        tick,
        ..
    } = app;

    // Keep the footer and at least a few rows of panels visible on a short
    // terminal; the informational panel shrinks first.
    let requirements_height = area.height.saturating_sub(3 + 1 + 4).min(6);
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(requirements_height),
        Constraint::Length(1),
    ])
    .split(area);

    let title = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("ZMK Studio", theme::title()),
            Span::styled(
                "  terminal keymap editor",
                Style::default().fg(theme::TEXT_DIM),
            ),
        ]),
        Line::from(Span::styled(
            "Live keymap editing over USB or Bluetooth LE",
            theme::dim(),
        )),
    ]);
    frame.render_widget(title, chunks[0]);

    let halves = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);
    render_usb_panel(frame, halves[0], connect, hits);
    render_bluetooth_panel(frame, halves[1], connect, *tick, hits);

    render_requirements(frame, chunks[2]);

    render_footer(
        frame,
        chunks[3],
        status.as_ref(),
        busy.as_deref(),
        *tick,
        "Tab switch · ↑↓ select · Enter connect · r rescan · q quit",
    );
}

fn panel(title: Line<'static>, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border(focused))
        .title(title)
        .style(theme::panel())
}

fn render_usb_panel(
    frame: &mut Frame,
    area: Rect,
    connect: &mut crate::tui::app::ConnectState,
    hits: &mut MouseMap,
) {
    let focused = connect.focus == Panel::Usb;
    let count = connect.usb.len();
    let title = Line::from(vec![
        Span::styled(
            " USB / HID ",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{count} device{}", if count == 1 { "" } else { "s" }),
            theme::dim(),
        ),
    ]);
    let block = panel(title, focused);
    let inner = block.inner(area);
    hits.add(area, MouseTarget::ConnectPanel(Panel::Usb));
    frame.render_widget(block, area);

    if connect.usb.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "No USB keyboards found",
                    Style::default().fg(theme::TEXT),
                )),
                Line::from(Span::styled(
                    "Studio uses a serial port; Vial uses raw HID.",
                    theme::dim(),
                )),
                Line::from(Span::styled(
                    "A 2.4G receiver may not expose the Vial raw-HID endpoint.",
                    theme::dim(),
                )),
            ])
            .wrap(Wrap { trim: true })
            .style(theme::panel()),
            inner,
        );
        return;
    }

    normalize(&mut connect.usb_state, count);
    let items: Vec<ListItem> = connect
        .usb
        .iter()
        .map(|device| {
            ListItem::new(Line::from(vec![Span::styled(
                text::ellipsize(&device.label(), inner.width.saturating_sub(2) as usize),
                Style::default().fg(theme::TEXT),
            )]))
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items)
            .style(theme::panel())
            .highlight_style(theme::selected())
            .highlight_symbol("› "),
        inner,
        &mut connect.usb_state,
    );
    hits.add_list(ListHit {
        kind: ListKind::UsbDevices,
        area: inner,
        offset: connect.usb_state.offset(),
        len: count,
    });
}

fn render_bluetooth_panel(
    frame: &mut Frame,
    area: Rect,
    connect: &mut crate::tui::app::ConnectState,
    tick: u64,
    hits: &mut MouseMap,
) {
    let focused = connect.focus == Panel::Bluetooth;
    let title = Line::from(vec![
        Span::styled(
            " Bluetooth ",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        if connect.scanning {
            Span::styled(format!("{} scanning", spinner(tick)), theme::dim())
        } else {
            Span::styled(
                format!(
                    "{} device{}",
                    connect.bluetooth.len(),
                    if connect.bluetooth.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                ),
                theme::dim(),
            )
        },
    ]);
    let block = panel(title, focused);
    let inner = block.inner(area);
    hits.add(area, MouseTarget::ConnectPanel(Panel::Bluetooth));
    frame.render_widget(block, area);

    let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
    if connect.bluetooth.is_empty() {
        let message = if let Some(error) = &connect.scan_error {
            vec![
                Line::from(Span::styled(
                    "Scan failed",
                    Style::default().fg(theme::DANGER),
                )),
                Line::from(Span::styled(
                    text::ellipsize(error, parts[0].width as usize),
                    theme::dim(),
                )),
            ]
        } else if connect.scanning {
            vec![Line::from(Span::styled(
                "Looking for keyboards…",
                theme::dim(),
            ))]
        } else {
            vec![
                Line::from(Span::styled(
                    "No Studio keyboards found",
                    Style::default().fg(theme::TEXT),
                )),
                Line::from(Span::styled(
                    "Connect the keyboard to this computer first.",
                    theme::dim(),
                )),
            ]
        };
        frame.render_widget(
            Paragraph::new(message)
                .wrap(Wrap { trim: true })
                .style(theme::panel()),
            parts[0],
        );
    } else {
        normalize(&mut connect.bluetooth_state, connect.bluetooth.len());
        let items: Vec<ListItem> = connect
            .bluetooth
            .iter()
            .map(|device| {
                let status = if device.connected {
                    "  connected"
                } else {
                    "  advertising"
                };
                let name = format!("{} (ZMK Studio)", device.name);
                let budget = (parts[0].width as usize).saturating_sub(4 + text::width(status));
                ListItem::new(Line::from(vec![
                    Span::styled(
                        text::ellipsize(&name, budget),
                        Style::default().fg(theme::TEXT),
                    ),
                    Span::styled(status, theme::dim()),
                ]))
            })
            .collect();
        frame.render_stateful_widget(
            List::new(items)
                .style(theme::panel())
                .highlight_style(theme::selected())
                .highlight_symbol("› "),
            parts[0],
            &mut connect.bluetooth_state,
        );
        hits.add_list(ListHit {
            kind: ListKind::BluetoothDevices,
            area: parts[0],
            offset: connect.bluetooth_state.offset(),
            len: connect.bluetooth.len(),
        });
    }

    let status = if connect.scanning {
        Line::from(vec![
            Span::styled(
                format!("{} ", spinner(tick)),
                Style::default().fg(theme::ACCENT),
            ),
            Span::styled("Scanning…", theme::dim()),
        ])
    } else if let Some(error) = &connect.scan_error {
        Line::from(Span::styled(
            text::ellipsize(error, parts[1].width as usize),
            Style::default().fg(theme::DANGER),
        ))
    } else {
        Line::from(Span::styled("Press r to scan again", theme::dim()))
    };
    frame.render_widget(Paragraph::new(status), parts[1]);
}

fn render_requirements(frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border(false))
        .title(Span::styled(" Before you connect ", theme::dim()))
        .style(theme::panel());
    let text = vec![
        Line::from(vec![
            Span::styled("Firmware  ", Style::default().fg(theme::ACCENT)),
            Span::styled(
                "build with CONFIG_ZMK_STUDIO=y.",
                Style::default().fg(theme::TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("USB       ", Style::default().fg(theme::ACCENT)),
            Span::styled(
                "add the studio-rpc-usb-uart snippet to your build.",
                Style::default().fg(theme::TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("Bluetooth ", Style::default().fg(theme::ACCENT)),
            Span::styled(
                "connect the keyboard to this computer first.",
                Style::default().fg(theme::TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("Port busy ", Style::default().fg(theme::ACCENT)),
            Span::styled(
                "close ZMK Studio or another app using the serial port, then retry.",
                Style::default().fg(theme::TEXT),
            ),
        ]),
    ];
    // Only show the lines that fit, ellipsized to the panel width, instead of
    // silently clipping wrapped text on a short terminal.
    let inner_height = block.inner(area).height as usize;
    let inner_width = block.inner(area).width as usize;
    let visible: Vec<Line> = text
        .into_iter()
        .take(inner_height)
        .map(|line| {
            let mut spans = Vec::new();
            let mut used = 0usize;
            for span in line.spans {
                let content = text::ellipsize(&span.content, inner_width.saturating_sub(used));
                used += text::width(&content);
                spans.push(Span::styled(content, span.style));
                if used >= inner_width {
                    break;
                }
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(
        Paragraph::new(visible).block(block).style(theme::panel()),
        area,
    );
}

// --------------------------------------------------------------------------
// Modals
// --------------------------------------------------------------------------

fn render_modal(frame: &mut Frame, area: Rect, app: &mut App, hits: &mut MouseMap) {
    let tick = app.tick;
    let (Some(editor), Some(modal)) = (app.editor.as_ref(), app.modal.as_mut()) else {
        return;
    };
    match modal {
        Modal::Binding(wizard) => render_binding_modal(frame, area, editor, wizard, hits),
        Modal::Layers(layers) => render_layer_modal(frame, area, editor, layers, hits),
        Modal::Layouts(layouts) => render_layout_modal(frame, area, editor, layouts, hits),
        Modal::Confirm(confirm) => render_confirm(frame, area, confirm, hits),
        Modal::Unlock(unlock) => render_unlock(frame, area, editor, unlock.waiting, tick, hits),
        Modal::Help => render_help(frame, area, hits),
        Modal::Warnings(warnings) => render_warnings(frame, area, warnings),
    }
}

fn modal_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn modal_block(title: impl Into<Line<'static>>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border(true))
        .title(title)
        .style(theme::panel())
}

fn inset(rect: Rect) -> Rect {
    if rect.width <= 2 {
        rect
    } else {
        Rect {
            x: rect.x + 1,
            width: rect.width - 2,
            ..rect
        }
    }
}

fn normalize(state: &mut ListState, len: usize) {
    if len == 0 {
        state.select(None);
        return;
    }
    let index = state.selected().unwrap_or(0).min(len - 1);
    state.select(Some(index));
    if *state.offset_mut() >= len {
        *state.offset_mut() = 0;
    }
}

fn render_binding_modal(
    frame: &mut Frame,
    area: Rect,
    editor: &EditorState,
    wizard: &mut BindingWizard,
    hits: &mut MouseMap,
) {
    let width = area.width.saturating_sub(4).clamp(34, 96);
    let desired_height = match wizard.stage {
        Stage::Behavior => 22,
        Stage::Review => 17,
        Stage::Param1 | Stage::Param2 => 30,
    };
    let height = desired_height.min(area.height.saturating_sub(2));
    let rect = modal_rect(area, width, height);
    frame.render_widget(Clear, rect);

    // Resolve the layer from the ID the wizard captured, so a layer reorder
    // while the wizard is open cannot mislabel the target.
    let layer_name = editor
        .keymap
        .layer_label_by_id(wizard.layer_id)
        .unwrap_or_else(|| format!("#{}", wizard.layer_id));
    let title = Line::from(vec![
        Span::styled(" Edit binding ", theme::title()),
        Span::styled(
            format!("· {layer_name} · position {} ", wizard.position),
            theme::dim(),
        ),
    ]);
    let block = modal_block(title);
    let inner = inset(block.inner(rect));
    frame.render_widget(block, rect);

    let chunks = Layout::vertical([
        Constraint::Length(1), // current
        Constraint::Length(1), // breadcrumb
        Constraint::Length(1), // context
        Constraint::Length(1), // filter or numeric value
        Constraint::Min(3),    // list / summary
        Constraint::Length(1), // hints
    ])
    .split(inner);

    let current = wizard.current_binding(&editor.keymap);
    let current_label = labels::binding_label(&editor.catalog, &editor.keymap, current);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Current  ", theme::dim()),
            Span::styled(current_label.detail, Style::default().fg(theme::TEXT)),
        ])),
        chunks[0],
    );

    let breadcrumb = match wizard.stage {
        Stage::Behavior => Line::from(vec![
            Span::styled(" Behavior ", theme::selected()),
            Span::styled("  ›  parameter 1  ›  parameter 2  ›  review", theme::dim()),
        ]),
        Stage::Param1 => Line::from(vec![
            Span::styled(" Behavior ", theme::dim()),
            Span::styled("  ›  ", theme::dim()),
            Span::styled(" Parameter 1 ", theme::selected()),
            Span::styled("  ›  parameter 2  ›  review", theme::dim()),
        ]),
        Stage::Param2 => Line::from(vec![
            Span::styled(" Behavior ", theme::dim()),
            Span::styled("  ›  parameter 1  ›  ", theme::dim()),
            Span::styled(" Parameter 2 ", theme::selected()),
            Span::styled("  ›  review", theme::dim()),
        ]),
        Stage::Review => Line::from(vec![
            Span::styled(
                " Behavior  ›  parameter 1  ›  parameter 2  ›  ",
                theme::dim(),
            ),
            Span::styled(" Review ", theme::selected()),
        ]),
    };
    frame.render_widget(Paragraph::new(breadcrumb), chunks[1]);

    match wizard.stage {
        Stage::Behavior => {
            let items = behavior_items(&editor.catalog, &wizard.behavior_filter);
            normalize(&mut wizard.behavior_state, items.len());
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" Choose the behavior this key should send", theme::dim()),
                    Span::styled(format!("   {} available", items.len()), theme::dim()),
                ])),
                chunks[2],
            );
            render_filter_line(frame, chunks[3], &wizard.behavior_filter);

            if items.is_empty() {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        "No matching behaviors. If the list is empty, unlock the keyboard first.",
                        theme::dim(),
                    ))
                    .wrap(Wrap { trim: true }),
                    chunks[4],
                );
            } else {
                let list_items: Vec<ListItem> = items
                    .iter()
                    .map(|behavior| {
                        let active = behavior.id == wizard.behavior;
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                if active { "● " } else { "  " },
                                Style::default().fg(theme::ACCENT),
                            ),
                            Span::styled(
                                behavior.display_name.clone(),
                                Style::default().fg(theme::TEXT),
                            ),
                            Span::styled(format!("   {}", param_count(behavior)), theme::dim()),
                        ]))
                    })
                    .collect();
                frame.render_stateful_widget(
                    List::new(list_items)
                        .style(theme::panel())
                        .highlight_style(theme::selected())
                        .highlight_symbol("› "),
                    chunks[4],
                    &mut wizard.behavior_state,
                );
                hits.add_list(ListHit {
                    kind: ListKind::Behaviors,
                    area: chunks[4],
                    offset: wizard.behavior_state.offset(),
                    len: items.len(),
                });
            }
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "↑↓ select · type to filter · Enter choose · Esc cancel",
                    theme::dim(),
                )),
                chunks[5],
            );
        }
        Stage::Param1 | Stage::Param2 => {
            let Some(desc) = param_desc(&editor.catalog, wizard.behavior, wizard.stage) else {
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        "This behavior has no parameters at this step.",
                        theme::dim(),
                    )),
                    chunks[4],
                );
                return;
            };
            let behavior_name = editor
                .catalog
                .name(wizard.behavior)
                .unwrap_or("Unknown behavior");
            let step = if wizard.stage == Stage::Param1 {
                "Parameter 1"
            } else {
                "Parameter 2"
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" Behavior  ", theme::dim()),
                    Span::styled(behavior_name.to_string(), Style::default().fg(theme::TEXT)),
                    Span::styled("   ·   ", theme::dim()),
                    Span::styled(
                        format!("{step} — {}", desc.name),
                        Style::default().fg(theme::ACCENT_ALT),
                    ),
                ])),
                chunks[2],
            );

            if let crate::model::ParamKind::Range { .. } = desc.kind {
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(" Value  ", theme::dim()),
                        Span::styled(
                            format!("{}", wizard.range_value),
                            Style::default()
                                .fg(theme::TEXT)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("    range {} … {}", wizard.range_min, wizard.range_max),
                            theme::dim(),
                        ),
                    ])),
                    chunks[3],
                );
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        "A numeric parameter. Type a number or use ↑↓ to adjust.",
                        theme::dim(),
                    ))
                    .alignment(Alignment::Center),
                    chunks[4],
                );
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        "↑↓ adjust · type digits · Enter next · Esc back",
                        theme::dim(),
                    )),
                    chunks[5],
                );
            } else {
                render_filter_line(frame, chunks[3], &wizard.param_filter);
                let items = param_items(
                    &editor.keymap,
                    &editor.catalog,
                    wizard.behavior,
                    wizard.stage,
                    &wizard.param_filter,
                );
                normalize(&mut wizard.param_state, items.len());
                if items.is_empty() {
                    frame.render_widget(
                        Paragraph::new(Span::styled("No matching values", theme::dim())),
                        chunks[4],
                    );
                } else {
                    let list_items: Vec<ListItem> = items
                        .iter()
                        .map(|item| {
                            ListItem::new(Line::from(vec![
                                Span::styled(
                                    text::fit_padded(&item.label, 6),
                                    Style::default()
                                        .fg(theme::TEXT)
                                        .add_modifier(Modifier::BOLD),
                                ),
                                Span::styled("  ", theme::dim()),
                                Span::styled(item.hint.clone(), theme::dim()),
                            ]))
                        })
                        .collect();
                    frame.render_stateful_widget(
                        List::new(list_items)
                            .style(theme::panel())
                            .highlight_style(theme::selected())
                            .highlight_symbol("› "),
                        chunks[4],
                        &mut wizard.param_state,
                    );
                    hits.add_list(ListHit {
                        kind: ListKind::Params,
                        area: chunks[4],
                        offset: wizard.param_state.offset(),
                        len: items.len(),
                    });
                }
                frame.render_widget(
                    Paragraph::new(Span::styled(
                        "↑↓ select · type to filter · Enter next · Esc back",
                        theme::dim(),
                    )),
                    chunks[5],
                );
            }
        }
        Stage::Review => {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" Review ", theme::selected()),
                    Span::styled("  press Enter to apply this binding", theme::dim()),
                ])),
                chunks[2],
            );

            let set = param_set(&editor.catalog, wizard.behavior);
            let mut lines = Vec::new();
            if let Some(desc) = set.and_then(|set| set.param1.first()) {
                lines.push(Line::from(vec![
                    Span::styled(" Parameter 1  ", theme::dim()),
                    Span::styled(desc.name.clone(), Style::default().fg(theme::TEXT)),
                    Span::styled("  =  ", theme::dim()),
                    Span::styled(
                        describe_param(editor, wizard.param1, wizard.behavior, Stage::Param1),
                        Style::default().fg(theme::ACCENT_ALT),
                    ),
                ]));
            }
            if let Some(desc) = set.and_then(|set| set.param2.first()) {
                lines.push(Line::from(vec![
                    Span::styled(" Parameter 2  ", theme::dim()),
                    Span::styled(desc.name.clone(), Style::default().fg(theme::TEXT)),
                    Span::styled("  =  ", theme::dim()),
                    Span::styled(
                        describe_param(editor, wizard.param2, wizard.behavior, Stage::Param2),
                        Style::default().fg(theme::ACCENT_ALT),
                    ),
                ]));
            }
            let preview =
                labels::binding_label(&editor.catalog, &editor.keymap, wizard.pending_binding());
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(" Preview  ", theme::dim()),
                Span::styled(
                    format!("{}  {}", preview.header, preview.main)
                        .trim()
                        .to_string(),
                    Style::default()
                        .fg(theme::TEXT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("   ", theme::dim()),
                Span::styled(preview.detail, Style::default().fg(theme::TEXT_DIM)),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "The change is applied to the keyboard immediately; press s in the editor to save it.",
                theme::dim(),
            )));
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), chunks[4]);
            let apply_label = "[ Apply ]";
            hits.add(
                Rect {
                    x: chunks[5].x,
                    y: chunks[5].y,
                    width: (text::width(apply_label) as u16).min(chunks[5].width),
                    height: 1,
                },
                MouseTarget::BindingApply,
            );
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(apply_label, theme::selected()),
                    Span::styled("  ·  Esc back", theme::dim()),
                ])),
                chunks[5],
            );
        }
    }
}

fn param_count(behavior: &crate::model::Behavior) -> String {
    let count = behavior
        .primary_param_set()
        .map(|set| {
            [set.param1.first(), set.param2.first()]
                .iter()
                .filter(|desc| {
                    desc.map(|desc| !matches!(desc.kind, crate::model::ParamKind::Nil))
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);
    match count {
        0 => "no parameters".into(),
        1 => "1 parameter".into(),
        n => format!("{n} parameters"),
    }
}

fn render_filter_line(frame: &mut Frame, area: Rect, filter: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Filter  ", theme::dim()),
            Span::styled(
                if filter.is_empty() {
                    "type to search".to_string()
                } else {
                    filter.to_string()
                },
                if filter.is_empty() {
                    Style::default().fg(theme::TEXT_DIM)
                } else {
                    Style::default().fg(theme::TEXT)
                },
            ),
            Span::styled("█", Style::default().fg(theme::ACCENT)),
        ])),
        area,
    );
}

fn describe_param(editor: &EditorState, value: u32, behavior_id: u32, stage: Stage) -> String {
    let Some(desc) = param_desc(&editor.catalog, behavior_id, stage) else {
        return value.to_string();
    };
    match desc.kind {
        crate::model::ParamKind::HidUsage { .. } => keycodes::describe(value),
        crate::model::ParamKind::LayerId => editor
            .keymap
            .layer_index_by_id(value)
            .map(|index| format!("{index} {}", editor.keymap.layers[index].label(index)))
            .unwrap_or_else(|| format!("#{value}")),
        crate::model::ParamKind::Constant(_) => param_set(&editor.catalog, behavior_id)
            .and_then(|set| {
                set.param1
                    .iter()
                    .chain(set.param2.iter())
                    .find(|param| {
                        matches!(param.kind, crate::model::ParamKind::Constant(c) if c == value)
                    })
                    .map(|param| param.name.clone())
            })
            .unwrap_or_else(|| value.to_string()),
        _ => value.to_string(),
    }
}

fn render_layer_modal(
    frame: &mut Frame,
    area: Rect,
    editor: &EditorState,
    modal: &mut LayerModal,
    hits: &mut MouseMap,
) {
    let width = area.width.saturating_sub(6).clamp(40, 84);
    let height = (editor.keymap.layers.len() as u16 + 7)
        .clamp(9, 24)
        .min(area.height.saturating_sub(2));
    let rect = modal_rect(area, width, height);
    frame.render_widget(Clear, rect);
    let free = editor.keymap.available_layers;
    let block = modal_block(Line::from(vec![
        Span::styled(" Layers ", theme::title()),
        Span::styled(format!("· {} ", editor.keymap.layers.len()), theme::dim()),
        Span::styled(
            if free == 0 {
                "· full ".to_string()
            } else {
                format!("· {free} free ")
            },
            theme::dim(),
        ),
    ]));
    let inner = inset(block.inner(rect));
    frame.render_widget(block, rect);

    let parts = Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).split(inner);
    normalize(&mut modal.state, editor.keymap.layers.len());
    let items: Vec<ListItem> = editor
        .keymap
        .layers
        .iter()
        .enumerate()
        .map(|(index, layer)| {
            let active = index == editor.layer_index;
            let suffix = format!("   id {} · {} keys", layer.id, layer.bindings.len());
            // highlight symbol + marker + index + suffix
            let budget = (parts[0].width as usize).saturating_sub(2 + 2 + 4 + text::width(&suffix));
            ListItem::new(Line::from(vec![
                Span::styled(
                    if active { "● " } else { "  " },
                    Style::default().fg(theme::ACCENT),
                ),
                Span::styled(format!("{index:>2}  "), theme::dim()),
                Span::styled(
                    text::ellipsize(&layer.label(index), budget),
                    Style::default().fg(theme::TEXT).add_modifier(if active {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
                ),
                Span::styled(suffix, theme::dim()),
            ]))
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items)
            .style(theme::panel())
            .highlight_style(theme::selected())
            .highlight_symbol("› "),
        parts[0],
        &mut modal.state,
    );
    hits.add_list(ListHit {
        kind: ListKind::Layers,
        area: parts[0],
        offset: modal.state.offset(),
        len: editor.keymap.layers.len(),
    });

    if let Some(rename) = &modal.renaming {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" Rename  ", Style::default().fg(theme::ACCENT)),
                Span::styled(rename.value.clone(), Style::default().fg(theme::TEXT)),
                Span::styled("█", Style::default().fg(theme::ACCENT)),
                Span::styled(format!("   (max {})", rename.max_len), theme::dim()),
            ])),
            parts[1],
        );
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "a add · r rename · x remove · J/K move · Enter switch · Esc close",
                theme::dim(),
            ))),
            parts[1],
        );
    }
}

fn render_layout_modal(
    frame: &mut Frame,
    area: Rect,
    editor: &EditorState,
    modal: &mut LayoutModal,
    hits: &mut MouseMap,
) {
    let width = area.width.saturating_sub(8).clamp(40, 72);
    let height = (editor.layouts.layouts.len() as u16 + 7)
        .clamp(9, 20)
        .min(area.height.saturating_sub(2));
    let rect = modal_rect(area, width, height);
    frame.render_widget(Clear, rect);
    let block = modal_block(Span::styled(" Physical layout ", theme::title()));
    let inner = inset(block.inner(rect));
    frame.render_widget(block, rect);
    let parts = Layout::vertical([Constraint::Min(2), Constraint::Length(1)]).split(inner);

    normalize(&mut modal.state, editor.layouts.layouts.len());
    let items: Vec<ListItem> = editor
        .layouts
        .layouts
        .iter()
        .enumerate()
        .map(|(index, layout)| {
            let active = index == editor.layouts.active_index;
            let suffix = format!("   {} keys", layout.keys.len());
            // highlight symbol + marker + suffix
            let budget = (parts[0].width as usize).saturating_sub(2 + 2 + text::width(&suffix));
            ListItem::new(Line::from(vec![
                Span::styled(
                    if active { "● " } else { "  " },
                    Style::default().fg(theme::ACCENT),
                ),
                Span::styled(
                    text::ellipsize(&layout.name, budget),
                    Style::default().fg(theme::TEXT),
                ),
                Span::styled(suffix, theme::dim()),
            ]))
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items)
            .style(theme::panel())
            .highlight_style(theme::selected())
            .highlight_symbol("› "),
        parts[0],
        &mut modal.state,
    );
    hits.add_list(ListHit {
        kind: ListKind::Layouts,
        area: parts[0],
        offset: modal.state.offset(),
        len: editor.layouts.layouts.len(),
    });
    frame.render_widget(
        Paragraph::new(Span::styled("Enter select · Esc close", theme::dim())),
        parts[1],
    );
}

fn render_confirm(frame: &mut Frame, area: Rect, confirm: &ConfirmModal, hits: &mut MouseMap) {
    let width = area.width.saturating_sub(10).clamp(36, 68);
    let message_lines = confirm.message.lines().count().max(1) as u16;
    let choice_rows = confirm.choices.len() as u16;
    let height = (message_lines + choice_rows + 5)
        .clamp(9, 20)
        .min(area.height.saturating_sub(2));
    let rect = modal_rect(area, width, height);
    frame.render_widget(Clear, rect);
    let block = modal_block(Span::styled(format!(" {} ", confirm.title), theme::title()));
    let inner = inset(block.inner(rect));
    frame.render_widget(block, rect);
    let parts =
        Layout::vertical([Constraint::Min(1), Constraint::Length(choice_rows)]).split(inner);

    frame.render_widget(
        Paragraph::new(confirm.message.clone())
            .style(Style::default().fg(theme::TEXT))
            .wrap(Wrap { trim: true }),
        parts[0],
    );

    // One choice per row, so every choice stays visible and clickable even in
    // a narrow terminal where a single line would wrap.
    for (index, choice) in confirm.choices.iter().enumerate() {
        let row = Rect {
            y: parts[1].y.saturating_add(index as u16),
            height: 1,
            ..parts[1]
        };
        if row.y >= inner.bottom() {
            break;
        }
        let key = format!(" [{}] ", choice.key.to_uppercase());
        let label = choice.label.clone();
        let width = (text::width(&key) + text::width(&label)) as u16;
        if width > 0 {
            hits.add(
                Rect {
                    width: width.min(parts[1].width),
                    ..row
                },
                MouseTarget::ConfirmChoice(index),
            );
        }
        let line = Line::from(vec![
            Span::styled(
                key,
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(label, Style::default().fg(theme::TEXT)),
        ]);
        frame.render_widget(Paragraph::new(line), row);
    }
}

fn render_unlock(
    frame: &mut Frame,
    area: Rect,
    editor: &EditorState,
    waiting: bool,
    tick: u64,
    hits: &mut MouseMap,
) {
    let width = area.width.saturating_sub(12).clamp(40, 76);
    let height = 14.min(area.height.saturating_sub(2));
    let rect = modal_rect(area, width, height);
    frame.render_widget(Clear, rect);
    let block = modal_block(Span::styled(" Unlock ZMK Studio ", theme::title()));
    let inner = inset(block.inner(rect));
    frame.render_widget(block, rect);
    // Pin the action row so it stays visible however the instructions wrap.
    let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);

    let mut lines = vec![
        Line::from(Span::styled("🔒", Style::default().fg(theme::DANGER))),
        Line::from(""),
        Line::from(Span::styled(
            format!("{} is locked", editor.info.name),
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press the key or combo bound to &studio_unlock.",
            Style::default().fg(theme::TEXT),
        )),
        Line::from(Span::styled(
            "Editing is disabled until the keyboard is unlocked.",
            theme::dim(),
        )),
        Line::from(""),
    ];
    if waiting {
        lines.push(Line::from(Span::styled(
            format!("{}  Waiting for unlock…", spinner(tick)),
            Style::default().fg(theme::WARNING),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "Esc dismisses this dialog",
            theme::dim(),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        parts[0],
    );

    let action = Line::from(Span::styled(
        "u / Enter  check now",
        Style::default().fg(theme::ACCENT),
    ));
    hits.add(
        Rect {
            y: parts[1].y,
            height: 1,
            ..parts[1]
        },
        MouseTarget::UnlockCheck,
    );
    frame.render_widget(
        Paragraph::new(action).alignment(Alignment::Center),
        parts[1],
    );
}

fn render_warnings(frame: &mut Frame, area: Rect, modal: &mut crate::tui::modals::WarningsModal) {
    let width = area.width.saturating_sub(8).clamp(44, 84);
    let height = 20.min(area.height.saturating_sub(2));
    let rect = modal_rect(area, width, height);
    frame.render_widget(Clear, rect);
    let block = modal_block(Line::from(vec![
        Span::styled(" Warnings ", theme::title()),
        Span::styled(format!("· {} ", modal.warnings.len()), theme::dim()),
    ]));
    let inner = inset(block.inner(rect));
    frame.render_widget(block, rect);

    let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
    let max_scroll = modal
        .warnings
        .len()
        .saturating_sub(parts[0].height as usize) as u16;
    modal.scroll = modal.scroll.min(max_scroll);
    let lines: Vec<Line> = modal
        .warnings
        .iter()
        .enumerate()
        .map(|(index, warning)| {
            Line::from(vec![
                Span::styled(format!("{:>2}  ", index + 1), theme::dim()),
                Span::styled(warning.clone(), Style::default().fg(theme::TEXT)),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((modal.scroll, 0))
            .wrap(Wrap { trim: false }),
        parts[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled("↑↓ scroll · Esc close", theme::dim())),
        parts[1],
    );
}

fn render_help(frame: &mut Frame, area: Rect, hits: &mut MouseMap) {
    let width = area.width.saturating_sub(8).clamp(44, 84);
    let height = 24.min(area.height.saturating_sub(2));
    let rect = modal_rect(area, width, height);
    frame.render_widget(Clear, rect);
    let block = modal_block(Span::styled(" Keys & notes ", theme::title()));
    let inner = inset(block.inner(rect));
    frame.render_widget(block, rect);

    let lines = vec![
        Line::from(Span::styled("Navigation", theme::title())),
        Line::from("  ↑ ↓ ← →  /  h j k l     move between keys"),
        Line::from("  Tab / Shift-Tab          next / previous layer"),
        Line::from("  0 – 9                    jump to layer"),
        Line::from("  g / G                    first / last key"),
        Line::from(Span::styled("Editing", theme::title())),
        Line::from("  Enter                    edit the selected binding"),
        Line::from("  L                        layers (add / rename / remove / move)"),
        Line::from("  p                        choose physical layout"),
        Line::from("  u                        unlock instructions"),
        Line::from("  W                        warnings reported while loading"),
        Line::from(Span::styled("Changes", theme::title())),
        Line::from("  s                        save changes to the keyboard"),
        Line::from("  d                        discard unsaved changes"),
        Line::from("  R                        restore stock keymap / settings"),
        Line::from("  b                        back to the device list"),
        Line::from("  q / Ctrl-C               quit"),
        Line::from(Span::styled("Notes", theme::title())),
        Line::from("  Edits apply live; only “save” persists them."),
        Line::from("  Saved settings override the compiled keymap; R restores the default."),
        Line::from("  Studio is locked until &studio_unlock; BLE needs a host connection."),
        Line::from(Span::styled(
            "Mouse: click select · double-click activate · wheel scroll · Esc/? close",
            theme::dim(),
        )),
    ];
    hits.add(inner, MouseTarget::HelpClose);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme::TEXT))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_rect_is_centered_and_clamped() {
        let area = Rect::new(0, 0, 80, 24);
        let rect = modal_rect(area, 40, 10);
        assert_eq!(rect.width, 40);
        assert_eq!(rect.height, 10);
        assert_eq!(rect.x, 20);
        assert_eq!(rect.y, 7);

        let huge = modal_rect(area, 200, 200);
        assert_eq!(huge, area);
    }

    #[test]
    fn spinner_cycles() {
        assert_eq!(spinner(0), SPINNER[0]);
        assert_eq!(spinner(100), SPINNER[2]);
    }
}
