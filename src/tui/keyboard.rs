//! Keyboard grid rendering.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::keycodes;
use crate::labels;
use crate::layout::{self, Viewport};
use crate::model::Binding;
use crate::tui::app::EditorState;
use crate::tui::mouse::{MouseMap, MouseTarget};
use crate::tui::{text, theme};

pub fn render(frame: &mut Frame, area: Rect, editor: &EditorState, hits: &mut MouseMap) {
    let layout = editor.effective_layout();
    // Reserve a small margin so split boards do not touch the terminal edges.
    let grid = layout::compute(
        &layout,
        Viewport {
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        },
    );
    if grid.is_empty() {
        let message = ratatui::widgets::Paragraph::new("No key positions reported by the keyboard")
            .style(theme::dim())
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(message, area);
        return;
    }

    let (offset_x, offset_y) = scroll_offset(area, &grid, editor.selected);
    let margin_x = area.width.saturating_sub(grid.width) / 2;
    let margin_y = area.height.saturating_sub(grid.height) / 2;

    for (position, cell) in
        (0..grid.len()).filter_map(|position| grid.cell(position).map(|cell| (position, cell)))
    {
        // Signed placement: cells scrolled off the top/left edge must be
        // clipped away, not clamped onto the edge and drawn over other keys.
        let Some(clip) = place(area, cell, margin_x, margin_y, offset_x, offset_y) else {
            continue;
        };

        let binding = editor
            .layer()
            .and_then(|layer| layer.bindings.get(position))
            .copied()
            .unwrap_or_default();
        let selected = position == editor.selected;
        let (bg, fg, header_fg) = colors(editor, binding, selected);
        let label = labels::binding_label(&editor.catalog, &editor.keymap, binding);

        frame
            .buffer_mut()
            .set_style(clip, Style::default().bg(bg).fg(fg));
        hits.add(clip, MouseTarget::EditorKey(position));

        let header_style = Style::default()
            .bg(bg)
            .fg(header_fg)
            .add_modifier(if selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
        let main_style = Style::default().bg(bg).fg(fg).add_modifier(if selected {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });

        let width = clip.width as usize;
        if clip.height >= 2 && !label.header.is_empty() {
            let header = text::center(&label.header, width);
            frame
                .buffer_mut()
                .set_stringn(clip.x, clip.y, header, width, header_style);
            let main = text::center(&label.main, width);
            frame
                .buffer_mut()
                .set_stringn(clip.x, clip.y + 1, main, width, main_style);
        } else {
            let main = text::center(&label.main, width);
            let row = clip.y + clip.height.saturating_sub(1) / 2;
            frame
                .buffer_mut()
                .set_stringn(clip.x, row, main, width, main_style);
        }
    }
}

/// Project a grid cell into the terminal and clip it to `area`. Returns `None`
/// when the cell is completely outside the visible keyboard area.
fn place(
    area: Rect,
    cell: layout::CellRect,
    margin_x: u16,
    margin_y: u16,
    offset_x: u16,
    offset_y: u16,
) -> Option<Rect> {
    let left = i32::from(area.x) + i32::from(margin_x) + i32::from(cell.x) - i32::from(offset_x);
    let top = i32::from(area.y) + i32::from(margin_y) + i32::from(cell.y) - i32::from(offset_y);
    let right = left + i32::from(cell.width);
    let bottom = top + i32::from(cell.height);
    let clip_left = left.max(i32::from(area.x));
    let clip_top = top.max(i32::from(area.y));
    let clip_right = right.min(i32::from(area.right()));
    let clip_bottom = bottom.min(i32::from(area.bottom()));
    if clip_right <= clip_left || clip_bottom <= clip_top {
        return None;
    }
    Some(Rect {
        x: clip_left as u16,
        y: clip_top as u16,
        width: (clip_right - clip_left) as u16,
        height: (clip_bottom - clip_top) as u16,
    })
}

fn scroll_offset(area: Rect, grid: &layout::Grid, selected: usize) -> (u16, u16) {
    let Some(cell) = grid.cell(selected) else {
        return (0, 0);
    };
    let max_x = grid.width.saturating_sub(area.width);
    let max_y = grid.height.saturating_sub(area.height);
    let x = cell
        .x
        .saturating_sub(area.width.saturating_sub(cell.width) / 2)
        .min(max_x);
    let y = cell
        .y
        .saturating_sub(area.height.saturating_sub(cell.height) / 2)
        .min(max_y);
    (x, y)
}

fn colors(editor: &EditorState, binding: Binding, selected: bool) -> (Color, Color, Color) {
    if selected {
        return (theme::SELECTED_BG, theme::SELECTED_FG, theme::SELECTED_FG);
    }

    let bg = match editor.catalog.name(binding.behavior_id) {
        Some("Transparent") => theme::KEY_TRANS_BG,
        Some("None") => theme::KEY_NONE_BG,
        Some("Momentary Layer")
        | Some("Toggle Layer")
        | Some("Layer-Tap")
        | Some("Mod-Tap")
        | Some("Mod-Tap (Vial)")
        | Some("Modified Key")
        | Some("Sticky Key")
        | Some("Sticky Layer")
        | Some("Sticky Modifier")
        | Some("To Layer") => theme::KEY_HOLD_BG,
        Some("Key Press") => match keycodes::by_value(binding.param1).map(|code| code.category) {
            Some(keycodes::Category::Media) => theme::KEY_MEDIA_BG,
            _ => theme::KEY_BG,
        },
        Some("Vial Keycode") => theme::KEY_UNKNOWN_BG,
        Some(_) => theme::KEY_BG,
        None if binding.is_empty() => theme::KEY_NONE_BG,
        None => theme::KEY_UNKNOWN_BG,
    };
    (bg, theme::TEXT, theme::TEXT_DIM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_offset_keeps_selection_visible() {
        let area = Rect::new(0, 0, 10, 5);
        let layout = crate::model::PhysicalLayout {
            name: "test".into(),
            keys: (0..10)
                .map(|index| crate::model::KeyGeometry {
                    x: index as f32,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                    ..Default::default()
                })
                .collect(),
        };
        let grid = layout::compute(
            &layout,
            Viewport {
                width: 10,
                height: 5,
            },
        );
        let (x, _) = scroll_offset(area, &grid, 8);
        assert!(x > 0);
        let (x, _) = scroll_offset(area, &grid, 0);
        assert_eq!(x, 0);
    }
}
