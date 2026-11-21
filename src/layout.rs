//! Projection of a ZMK physical layout onto terminal cells, plus directional
//! selection movement for irregular (split, ortholinear, staggered) boards.

use crate::model::{KeyGeometry, PhysicalLayout};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Viewport {
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRect {
    /// Relative to the keyboard viewport origin.
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Default)]
pub struct Grid {
    cells: Vec<CellRect>,
    pub unit_cols: u16,
    pub unit_rows: u16,
    pub width: u16,
    pub height: u16,
}

impl Grid {
    pub fn cell(&self, position: usize) -> Option<CellRect> {
        self.cells.get(position).copied()
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Move the selection to the nearest key in `direction`.
    pub fn move_selection(
        &self,
        layout: &PhysicalLayout,
        position: usize,
        direction: Direction,
    ) -> Option<usize> {
        move_selection(layout, position, direction)
    }
}

/// Move the selection to the nearest key in `direction`.
///
/// Navigation uses the layout's key-unit centres, so it works for staggered,
/// column-staggered and split boards. Rotations are ignored for positioning.
pub fn move_selection(
    layout: &PhysicalLayout,
    position: usize,
    direction: Direction,
) -> Option<usize> {
    let current = layout.keys.get(position)?.center();
    let mut best: Option<(f32, usize)> = None;

    for (index, key) in layout.keys.iter().enumerate() {
        if index == position {
            continue;
        }
        let candidate = key.center();
        let dx = candidate.0 - current.0;
        let dy = candidate.1 - current.1;

        let (primary, secondary) = match direction {
            Direction::Left if dx < -0.01 => (-dx, dy.abs()),
            Direction::Right if dx > 0.01 => (dx, dy.abs()),
            Direction::Up if dy < -0.01 => (-dy, dx.abs()),
            Direction::Down if dy > 0.01 => (dy, dx.abs()),
            _ => continue,
        };

        // Prefer movement along the requested axis, then alignment with the
        // current row/column.
        let score = primary + secondary * 3.0 + if secondary > 0.75 { 0.5 } else { 0.0 };
        if best.is_none_or(|(best_score, _)| best_score > score) {
            best = Some((score, index));
        }
    }

    best.map(|(_, index)| index)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Lay out keys so that 1u is `unit_cols` columns and `unit_rows` rows wide.
pub fn compute(layout: &PhysicalLayout, viewport: Viewport) -> Grid {
    if layout.keys.is_empty() || viewport.width == 0 || viewport.height == 0 {
        return Grid::default();
    }

    let bounds_width = layout
        .keys
        .iter()
        .map(|key| key.x + key.width)
        .fold(0.0_f32, f32::max)
        .max(1.0);
    let bounds_height = layout
        .keys
        .iter()
        .map(|key| key.y + key.height)
        .fold(0.0_f32, f32::max)
        .max(1.0);

    let unit_cols = (viewport.width as f32 / bounds_width).floor() as u16;
    let unit_cols = unit_cols.clamp(3, 10);
    let unit_rows = (viewport.height as f32 / bounds_height).floor() as u16;
    let unit_rows = unit_rows.clamp(2, 4);

    let mut cells = Vec::with_capacity(layout.keys.len());
    let mut max_x = 0;
    let mut max_y = 0;
    for key in &layout.keys {
        let rect = project(key, unit_cols, unit_rows);
        max_x = max_x.max(rect.x + rect.width);
        max_y = max_y.max(rect.y + rect.height);
        cells.push(rect);
    }

    Grid {
        cells,
        unit_cols,
        unit_rows,
        width: max_x,
        height: max_y,
    }
}

fn project(key: &KeyGeometry, unit_cols: u16, unit_rows: u16) -> CellRect {
    let x = (key.x * unit_cols as f32).round() as u16;
    let y = (key.y * unit_rows as f32).round() as u16;
    let full_width = (key.width * unit_cols as f32).round().max(1.0) as u16;
    let full_height = (key.height * unit_rows as f32).round().max(1.0) as u16;
    CellRect {
        x,
        y,
        width: full_width.saturating_sub(1).max(1),
        height: full_height.saturating_sub(1).max(1),
    }
}

/// Find the key position nearest to a terminal cell, used for mouse-free
/// selection sanity checks and tests.
pub fn position_at(grid: &Grid, column: u16, row: u16) -> Option<usize> {
    grid.cells.iter().position(|cell| {
        column >= cell.x
            && column < cell.x + cell.width
            && row >= cell.y
            && row < cell.y + cell.height
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(x: f32, y: f32, width: f32, height: f32) -> KeyGeometry {
        KeyGeometry {
            x,
            y,
            width,
            height,
            ..Default::default()
        }
    }

    fn row() -> PhysicalLayout {
        PhysicalLayout {
            name: "row".into(),
            keys: vec![
                key(0.0, 0.0, 1.0, 1.0),
                key(1.0, 0.0, 1.0, 1.0),
                key(2.0, 0.0, 1.0, 1.0),
            ],
        }
    }

    #[test]
    fn projects_unit_key_to_cells() {
        let grid = compute(
            &row(),
            Viewport {
                width: 30,
                height: 9,
            },
        );
        assert_eq!(grid.unit_cols, 10);
        assert_eq!(grid.unit_rows, 4);
        assert_eq!(
            grid.cell(0),
            Some(CellRect {
                x: 0,
                y: 0,
                width: 9,
                height: 3
            })
        );
        assert_eq!(
            grid.cell(1),
            Some(CellRect {
                x: 10,
                y: 0,
                width: 9,
                height: 3
            })
        );
        assert_eq!(grid.width, 29);
        assert_eq!(grid.height, 3);
    }

    #[test]
    fn narrow_viewport_uses_minimum_unit_width() {
        let grid = compute(
            &row(),
            Viewport {
                width: 10,
                height: 6,
            },
        );
        // unit_cols is clamped to 3, so the grid intentionally overflows and
        // the view clips instead of producing unusable one-cell keys.
        assert_eq!(grid.unit_cols, 3);
        assert_eq!(grid.cell(0).unwrap().width, 2);
    }

    #[test]
    fn directional_navigation_follows_geometry() {
        let layout = PhysicalLayout {
            name: "split".into(),
            keys: vec![
                key(0.0, 0.0, 1.0, 1.0),
                key(1.0, 0.0, 1.0, 1.0),
                key(0.0, 1.0, 1.0, 1.0),
                key(1.0, 1.0, 1.0, 1.0),
                // thumb cluster, offset to the right and down
                key(1.5, 2.0, 1.0, 1.0),
            ],
        };
        let grid = compute(
            &layout,
            Viewport {
                width: 40,
                height: 20,
            },
        );
        assert_eq!(grid.move_selection(&layout, 0, Direction::Right), Some(1));
        assert_eq!(grid.move_selection(&layout, 0, Direction::Down), Some(2));
        assert_eq!(grid.move_selection(&layout, 1, Direction::Down), Some(3));
        assert_eq!(grid.move_selection(&layout, 3, Direction::Down), Some(4));
        assert_eq!(grid.move_selection(&layout, 4, Direction::Up), Some(3));
        assert_eq!(grid.move_selection(&layout, 0, Direction::Left), None);
        assert_eq!(grid.move_selection(&layout, 0, Direction::Up), None);
    }

    #[test]
    fn position_lookup_uses_cell_bounds() {
        let grid = compute(
            &row(),
            Viewport {
                width: 30,
                height: 9,
            },
        );
        assert_eq!(position_at(&grid, 0, 0), Some(0));
        assert_eq!(position_at(&grid, 10, 0), Some(1));
        assert_eq!(position_at(&grid, 9, 0), None); // gap between keys
        assert_eq!(position_at(&grid, 500, 0), None);
    }

    #[test]
    fn empty_layout_produces_empty_grid() {
        let grid = compute(
            &PhysicalLayout {
                name: "empty".into(),
                keys: vec![],
            },
            Viewport {
                width: 80,
                height: 24,
            },
        );
        assert!(grid.is_empty());
    }
}
