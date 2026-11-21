//! Mouse hit-testing map.
//!
//! Widgets are laid out during rendering, so the render pass records the
//! rectangles and list geometry that the mouse handler consumes on the next
//! event. Only the topmost screen (including any modal) contributes targets.

use ratatui::layout::Rect;

use crate::tui::app::Panel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseTarget {
    EditorKey(usize),
    LayerTab(usize),
    ConnectPanel(Panel),
    UsbDevice(usize),
    BluetoothDevice(usize),
    BindingBehavior(usize),
    BindingParam(usize),
    LayerRow(usize),
    LayoutRow(usize),
    ConfirmChoice(usize),
    BindingApply,
    EditorSave,
    EditorDiscard,
    EditorQuit,
    HelpClose,
    UnlockCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    UsbDevices,
    BluetoothDevices,
    Behaviors,
    Params,
    Layers,
    Layouts,
}

#[derive(Debug, Clone, Copy)]
pub struct ListHit {
    pub kind: ListKind,
    pub area: Rect,
    pub offset: usize,
    pub len: usize,
}

impl ListHit {
    pub fn target(&self, index: usize) -> Option<MouseTarget> {
        if index >= self.len {
            return None;
        }
        Some(match self.kind {
            ListKind::UsbDevices => MouseTarget::UsbDevice(index),
            ListKind::BluetoothDevices => MouseTarget::BluetoothDevice(index),
            ListKind::Behaviors => MouseTarget::BindingBehavior(index),
            ListKind::Params => MouseTarget::BindingParam(index),
            ListKind::Layers => MouseTarget::LayerRow(index),
            ListKind::Layouts => MouseTarget::LayoutRow(index),
        })
    }
}

#[derive(Debug, Default)]
pub struct MouseMap {
    regions: Vec<(Rect, MouseTarget)>,
    lists: Vec<ListHit>,
}

impl MouseMap {
    pub fn add(&mut self, area: Rect, target: MouseTarget) {
        if area.width > 0 && area.height > 0 {
            self.regions.push((area, target));
        }
    }

    pub fn add_list(&mut self, hit: ListHit) {
        if hit.area.width > 0 && hit.area.height > 0 && hit.len > 0 {
            self.lists.push(hit);
        }
    }

    /// Find the target at a terminal cell. List rows take precedence over
    /// background regions such as a whole panel.
    pub fn hit(&self, column: u16, row: u16) -> Option<MouseTarget> {
        if let Some((kind, index)) = self.index_at(column, row) {
            return match kind {
                ListKind::UsbDevices => Some(MouseTarget::UsbDevice(index)),
                ListKind::BluetoothDevices => Some(MouseTarget::BluetoothDevice(index)),
                ListKind::Behaviors => Some(MouseTarget::BindingBehavior(index)),
                ListKind::Params => Some(MouseTarget::BindingParam(index)),
                ListKind::Layers => Some(MouseTarget::LayerRow(index)),
                ListKind::Layouts => Some(MouseTarget::LayoutRow(index)),
            };
        }
        self.regions
            .iter()
            .rev()
            .find(|(area, _)| contains(*area, column, row))
            .map(|(_, target)| *target)
    }

    /// The list under the pointer, used for wheel scrolling.
    pub fn list_at(&self, column: u16, row: u16) -> Option<ListKind> {
        self.lists
            .iter()
            .rev()
            .find(|list| contains(list.area, column, row))
            .map(|list| list.kind)
    }

    pub fn list(&self, kind: ListKind) -> Option<ListHit> {
        self.lists.iter().find(|list| list.kind == kind).copied()
    }

    /// Rectangle of a visible list item, for tests and tooling.
    pub fn item_rect(&self, kind: ListKind, index: usize) -> Option<Rect> {
        let list = self.list(kind)?;
        if index < list.offset || index >= list.offset + list.area.height as usize {
            return None;
        }
        Some(Rect {
            x: list.area.x,
            y: list.area.y + (index - list.offset) as u16,
            width: list.area.width,
            height: 1,
        })
    }

    /// Rectangle of a non-list target, for tests and tooling.
    pub fn target_rect(&self, target: MouseTarget) -> Option<Rect> {
        self.regions
            .iter()
            .rev()
            .find(|(_, candidate)| *candidate == target)
            .map(|(area, _)| *area)
    }

    fn index_at(&self, column: u16, row: u16) -> Option<(ListKind, usize)> {
        for list in self.lists.iter().rev() {
            if contains(list.area, column, row) {
                let index = list.offset + (row - list.area.y) as usize;
                if index < list.len {
                    return Some((list.kind, index));
                }
            }
        }
        None
    }
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_rows_win_over_background_regions() {
        let mut map = MouseMap::default();
        map.add(
            Rect::new(0, 0, 20, 10),
            MouseTarget::ConnectPanel(Panel::Usb),
        );
        map.add_list(ListHit {
            kind: ListKind::UsbDevices,
            area: Rect::new(1, 1, 10, 5),
            offset: 0,
            len: 3,
        });
        assert_eq!(map.hit(2, 1), Some(MouseTarget::UsbDevice(0)));
        assert_eq!(map.hit(2, 3), Some(MouseTarget::UsbDevice(2)));
        // A row in the panel but below the device rows falls back to the panel.
        assert_eq!(map.hit(2, 7), Some(MouseTarget::ConnectPanel(Panel::Usb)));
        assert_eq!(map.hit(50, 50), None);
    }

    #[test]
    fn scrolled_lists_report_the_visible_index_and_rect() {
        let mut map = MouseMap::default();
        map.add_list(ListHit {
            kind: ListKind::Behaviors,
            area: Rect::new(0, 5, 20, 4),
            offset: 3,
            len: 10,
        });
        assert_eq!(map.hit(0, 5), Some(MouseTarget::BindingBehavior(3)));
        assert_eq!(map.hit(0, 8), Some(MouseTarget::BindingBehavior(6)));
        assert_eq!(
            map.item_rect(ListKind::Behaviors, 4),
            Some(Rect::new(0, 6, 20, 1))
        );
        assert_eq!(map.item_rect(ListKind::Behaviors, 1), None);
        assert_eq!(map.list_at(0, 6), Some(ListKind::Behaviors));
    }

    #[test]
    fn list_hit_maps_every_kind_and_rejects_out_of_range() {
        let area = Rect::new(0, 0, 10, 3);
        let cases = [
            (ListKind::UsbDevices, MouseTarget::UsbDevice(1)),
            (ListKind::BluetoothDevices, MouseTarget::BluetoothDevice(1)),
            (ListKind::Behaviors, MouseTarget::BindingBehavior(1)),
            (ListKind::Params, MouseTarget::BindingParam(1)),
            (ListKind::Layers, MouseTarget::LayerRow(1)),
            (ListKind::Layouts, MouseTarget::LayoutRow(1)),
        ];
        for (kind, expected) in cases {
            let hit = ListHit {
                kind,
                area,
                offset: 0,
                len: 2,
            };
            assert_eq!(hit.target(1), Some(expected));
            assert_eq!(hit.target(2), None);
        }
    }

    #[test]
    fn target_rect_finds_regions() {
        let mut map = MouseMap::default();
        map.add(Rect::new(3, 4, 5, 1), MouseTarget::EditorSave);
        assert_eq!(
            map.target_rect(MouseTarget::EditorSave),
            Some(Rect::new(3, 4, 5, 1))
        );
        assert_eq!(map.target_rect(MouseTarget::EditorQuit), None);
    }
}
