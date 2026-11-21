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

    /// Move the selection to the nearest key in `direction`, following the
    /// rendered grid rather than the raw (staggered) key centres so that a move
    /// between two keys drawn on the same row is rejected as "not up/down".
    pub fn move_selection(
        &self,
        _layout: &PhysicalLayout,
        position: usize,
        direction: Direction,
    ) -> Option<usize> {
        let current = self.cells.get(position)?;
        if self.cells.is_empty() || self.unit_rows == 0 || self.unit_cols == 0 {
            return None;
        }
        let (current_row, current_col) = self.row_column(current);
        let mut best: Option<(i32, i32, usize)> = None;

        for (index, cell) in self.cells.iter().enumerate() {
            if index == position {
                continue;
            }
            let (row, col) = self.row_column(cell);
            let row_delta = row - current_row;
            let col_delta = col - current_col;
            let (primary, secondary) = match direction {
                Direction::Left if col_delta < 0 => (-col_delta, row_delta.abs()),
                Direction::Right if col_delta > 0 => (col_delta, row_delta.abs()),
                Direction::Up if row_delta < 0 => (-row_delta, col_delta.abs()),
                Direction::Down if row_delta > 0 => (row_delta, col_delta.abs()),
                _ => continue,
            };
            // Nearest row/column first, then alignment with the current one.
            let score = (primary, secondary);
            if best.is_none_or(|(best_primary, best_secondary, _)| {
                score < (best_primary, best_secondary)
            }) {
                best = Some((score.0, score.1, index));
            }
        }

        best.map(|(_, _, index)| index)
    }

    /// Row and column index of a projected cell. Keys that share a rendered row
    /// (or column) get the same index even when their physical positions
    /// differ by column stagger.
    fn row_column(&self, cell: &CellRect) -> (i32, i32) {
        let center_x = i32::from(cell.x) + i32::from(cell.width) / 2;
        let center_y = i32::from(cell.y) + i32::from(cell.height) / 2;
        (
            center_y.div_euclid(i32::from(self.unit_rows)),
            center_x.div_euclid(i32::from(self.unit_cols)),
        )
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

    // Normalise signed origins so layouts with negative coordinates render
    // from the top-left instead of piling every negative key onto cell zero.
    let min_x = layout.keys.iter().map(|key| key.x).fold(0.0_f32, f32::min);
    let min_y = layout.keys.iter().map(|key| key.y).fold(0.0_f32, f32::min);
    let bounds_width = layout
        .keys
        .iter()
        .map(|key| key.x - min_x + key.width)
        .fold(0.0_f32, f32::max)
        .max(1.0);
    let bounds_height = layout
        .keys
        .iter()
        .map(|key| key.y - min_y + key.height)
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
        let rect = project(key, min_x, min_y, unit_cols, unit_rows);
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

fn project(key: &KeyGeometry, min_x: f32, min_y: f32, unit_cols: u16, unit_rows: u16) -> CellRect {
    let x = ((key.x - min_x) * unit_cols as f32).round() as u16;
    let y = ((key.y - min_y) * unit_rows as f32).round() as u16;
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
    fn negative_origins_are_normalised_into_the_grid() {
        let layout = PhysicalLayout {
            name: "negative".into(),
            keys: vec![
                key(-2.0, 0.0, 1.0, 1.0),
                key(-1.0, 0.0, 1.0, 1.0),
                key(0.0, 0.0, 1.0, 1.0),
            ],
        };
        let grid = compute(
            &layout,
            Viewport {
                width: 60,
                height: 10,
            },
        );
        let cells: Vec<CellRect> = (0..3).map(|i| grid.cell(i).unwrap()).collect();
        assert_eq!(cells[0].x, 0, "the leftmost key starts at column zero");
        assert!(cells[0].x < cells[1].x);
        assert!(cells[1].x < cells[2].x);
        assert_eq!(position_at(&grid, 0, 0), Some(0));
    }

    #[test]
    fn grid_navigation_ignores_column_stagger() {
        // Two keys drawn on the same rendered row but with 0.12u of stagger.
        let layout = PhysicalLayout {
            name: "stagger".into(),
            keys: vec![
                key(0.0, 0.12, 1.0, 1.0),
                key(1.0, 0.0, 1.0, 1.0),
                key(0.0, 1.12, 1.0, 1.0),
            ],
        };
        let grid = compute(
            &layout,
            Viewport {
                width: 40,
                height: 20,
            },
        );
        assert_eq!(
            grid.move_selection(&layout, 0, Direction::Up),
            None,
            "the staggered neighbour is on the same rendered row"
        );
        assert_eq!(
            grid.move_selection(&layout, 2, Direction::Up),
            Some(0),
            "a real row above keeps the same column"
        );
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
