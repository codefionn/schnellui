//! Write access to a mounted terminal's retained model ([`TerminalGrid::patched`]).
//!
//! Each setter records the damage it causes as it happens, so the frame never
//! diffs the grid: a row write dirties that row, a cursor move dirties the old
//! and new cursor rows, and the paint pass re-emits exactly those rows.

use super::interaction::{mark_cursor_row, mark_selection_rows};
use super::*;

/// What a patch changed beyond the per-row dirty sets kept on the state.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PatchEffects {
    pub(super) resized: bool,
}

/// A borrowed, write-enabled view of a [`TerminalGrid`]'s retained model, handed
/// to the closure of [`TerminalGrid::patched`].
///
/// Writes go straight into the widget's model. Nothing is cloned and nothing is
/// compared cell by cell afterwards, so the cost of an update is the rows it
/// touches. Mutate cells through [`row_mut`](Self::row_mut); every other field
/// has a setter that dirties only what changed.
pub struct TerminalGridPatch<'a> {
    state: &'a mut TerminalGridState,
    effects: PatchEffects,
}

impl<'a> TerminalGridPatch<'a> {
    pub(super) fn new(state: &'a mut TerminalGridState) -> Self {
        Self {
            state,
            effects: PatchEffects::default(),
        }
    }

    pub(super) fn effects(&self) -> PatchEffects {
        self.effects
    }
}

impl TerminalGridPatch<'_> {
    /// The retained model as it stands, including writes made earlier in this
    /// patch.
    pub fn model(&self) -> &TerminalGridModel {
        &self.state.model
    }

    /// Changes the grid dimensions. Cells inside both the old and new bounds keep
    /// their contents; new cells are blank in the model's default colors. A real
    /// change requests a layout pass and a full repaint (a grow event); the same
    /// dimensions are a no-op.
    pub fn resize(&mut self, columns: usize, rows: usize) {
        let model = &mut self.state.model;
        if model.columns == columns && model.rows == rows {
            return;
        }
        let blank = TerminalCell::blank(model.foreground, model.background);
        let mut cells = vec![blank; columns.saturating_mul(rows)];
        let keep_columns = columns.min(model.columns);
        for row in 0..rows.min(model.rows) {
            let from = row * model.columns;
            let to = row * columns;
            cells[to..to + keep_columns].clone_from_slice(&model.cells[from..from + keep_columns]);
        }
        model.cells = cells;
        model.columns = columns;
        model.rows = rows;
        self.state.mark_all_dirty();
        self.effects.resized = true;
    }

    /// Sets the default colors. A background change repaints the whole grid,
    /// because the backdrop primitive is shared by every row.
    pub fn set_colors(&mut self, foreground: Color, background: Color) {
        let model = &mut self.state.model;
        if model.foreground == foreground && model.background == background {
            return;
        }
        let background_changed = model.background != background;
        model.foreground = foreground;
        model.background = background;
        if background_changed {
            self.state.full_paint_dirty = true;
        }
        // Default colors only decide which background runs are skipped, so a
        // foreground-only change still rebuilds rows, but no backdrop.
        self.state.dirty_rows.reset_all(self.state.model.rows);
    }

    /// The cells of one row, for in-place writes. Marks that row's paint and
    /// accessible text dirty. A row outside the grid returns an empty slice.
    pub fn row_mut(&mut self, row: usize) -> &mut [TerminalCell] {
        let model = &mut self.state.model;
        if row >= model.rows {
            return &mut [];
        }
        self.state.dirty_rows.insert(row);
        self.state.a11y_rows.insert(row);
        let start = row * model.columns;
        &mut model.cells[start..start + model.columns]
    }

    /// Moves, restyles, shows or hides the cursor. Only a real change dirties
    /// anything: the old and the new cursor rows and the cursor fragment.
    pub fn set_cursor(&mut self, cursor: Option<TerminalCursor>) {
        let previous = self.state.model.cursor;
        if previous == cursor {
            return;
        }
        self.state.model.cursor = cursor;
        mark_cursor_row(self.state, previous);
        mark_cursor_row(self.state, cursor);
        self.state.cursor_dirty = true;
    }

    /// Replaces the selection, dirtying the rows of the old and new selection.
    pub fn set_selection(&mut self, selection: Option<TerminalSelection>) {
        let previous = self.state.model.selection;
        if previous == selection {
            return;
        }
        self.state.model.selection = selection;
        mark_selection_rows(self.state, previous);
        mark_selection_rows(self.state, selection);
    }

    /// Replaces the image placements. Images keep their atlas allocation when an
    /// id reappears with the same dimensions.
    pub fn set_images(&mut self, images: Vec<TerminalImage>) {
        self.state.model.images = images;
        self.state.images_dirty = true;
    }

    /// Mutable access to the image placements, for in-place edits that reuse the
    /// existing `Vec` and pixel buffers. Marks the images dirty.
    pub fn images_mut(&mut self) -> &mut Vec<TerminalImage> {
        self.state.images_dirty = true;
        &mut self.state.model.images
    }
}
