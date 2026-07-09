use super::*;

pub fn terminal_grid_metrics(
    runtime: &crate::Runtime,
    id: WidgetId,
) -> Option<TerminalCellMetrics> {
    runtime
        .terminal_grid
        .with(|grids| grids.borrow().get(id).map(|state| state.metrics))
}

/// Maps a window-space pointer to a mounted terminal cell and resolves its link.
pub fn terminal_grid_hit_test(
    runtime: &crate::Runtime,
    scene: &Scene,
    id: WidgetId,
    point: Point,
) -> Option<TerminalGridHit> {
    let rect = scene.layout(id)?.rect;
    runtime.terminal_grid.with(|grids| {
        let grids = grids.borrow();
        let state = grids.get(id)?;
        let position =
            state
                .metrics
                .point_to_cell(rect, point, state.model.columns, state.model.rows)?;
        Some(TerminalGridHit {
            position,
            hyperlink: state.model.hyperlink_at(position).map(str::to_owned),
        })
    })
}

/// Re-evaluates one ready signal-backed terminal source. Unlike versioned grids,
/// this is reached only through the runtime's targeted subscription queue.
pub(crate) fn poll_dynamic_source(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    id: WidgetId,
) -> bool {
    if scene.node(id).is_none() {
        return false;
    }
    let source = runtime.terminal_grid.with(|grids| {
        let mut grids = grids.borrow_mut();
        let state = grids.get_mut(id)?;
        match state.source.take()? {
            source @ DynamicTerminalGridSource::Unversioned(_) => Some(source),
            versioned => {
                state.source = Some(versioned);
                None
            }
        }
    });
    let Some(DynamicTerminalGridSource::Unversioned(mut source)) = source else {
        return false;
    };
    // Run/re-track without holding the terminal registry borrow.
    let current = runtime.track_dynamic(id, &mut source);
    apply_dynamic_model(
        runtime,
        scene,
        id,
        DynamicTerminalGridSource::Unversioned(source),
        current,
    );
    true
}

/// Polls externally-clocked terminal revisions. These sources intentionally do
/// not participate in signal tracking: a PTY or host event loop can advance a
/// revision without writing a SchnellUI signal.
pub(crate) fn poll_dynamic_sources(runtime: &crate::Runtime, scene: &mut Scene) {
    let ids = runtime.terminal_grid.take_ids(|state| {
        matches!(
            state.source.as_ref(),
            Some(DynamicTerminalGridSource::Versioned { .. })
        )
    });
    for &id in &ids {
        if scene.node(id).is_none() {
            continue;
        }
        let source = runtime.terminal_grid.with(|grids| {
            grids
                .borrow_mut()
                .get_mut(id)
                .and_then(|state| state.source.take())
        });
        let Some(source) = source else { continue };
        let (current, source) = match source {
            DynamicTerminalGridSource::Versioned {
                mut revision,
                observed_revision,
                mut model,
            } => {
                let current_revision = revision();
                if current_revision == observed_revision {
                    runtime.terminal_grid.with(|grids| {
                        if let Some(state) = grids.borrow_mut().get_mut(id) {
                            state.source = Some(DynamicTerminalGridSource::Versioned {
                                revision,
                                observed_revision,
                                model,
                            });
                        }
                    });
                    continue;
                }
                let current = model();
                (
                    current,
                    DynamicTerminalGridSource::Versioned {
                        revision,
                        observed_revision: current_revision,
                        model,
                    },
                )
            }
            DynamicTerminalGridSource::Unversioned(source) => {
                // The id snapshot contains only versioned states. Preserve this
                // defensively if a callback caused a structural mutation.
                runtime.terminal_grid.with(|grids| {
                    if let Some(state) = grids.borrow_mut().get_mut(id) {
                        state.source = Some(DynamicTerminalGridSource::Unversioned(source));
                    }
                });
                continue;
            }
        };
        apply_dynamic_model(runtime, scene, id, source, current);
    }
    runtime.terminal_grid.return_ids(ids);
}

fn apply_dynamic_model(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    id: WidgetId,
    source: DynamicTerminalGridSource,
    current: TerminalGridModel,
) {
    let changed = runtime.terminal_grid.with(|grids| {
        let mut grids = grids.borrow_mut();
        let state = grids.get_mut(id)?;
        state.source = Some(source);
        let dimensions_changed =
            state.model.columns != current.columns || state.model.rows != current.rows;
        let (text_changed, visual_changed) = if dimensions_changed {
            state.full_paint_dirty = true;
            state.dirty_rows = vec![true; current.rows];
            state.images_dirty = true;
            state.cursor_dirty = true;
            (true, true)
        } else {
            let diff = plan_incremental_paint(state, &current);
            (diff.text_changed, diff.visual_changed)
        };
        state.model = current;
        if dimensions_changed {
            *state.measured.borrow_mut() = state
                .metrics
                .grid_size(state.model.columns, state.model.rows);
        }
        (visual_changed || text_changed || dimensions_changed).then(|| {
            (
                text_changed.then(|| state.model.plain_text()),
                dimensions_changed,
                visual_changed,
            )
        })
    });
    if let Some((plain_text, dimensions_changed, visual_changed)) = changed {
        if let Some(plain_text) = plain_text {
            scene.set_a11y_value(id, Some(plain_text));
        }
        if dimensions_changed {
            scene.mark_dirty(id, DirtyFlags::LAYOUT);
        }
        if visual_changed {
            scene.mark_dirty(id, DirtyFlags::PAINT);
        }
    }
}

fn plain_text_cell_eq(old: Option<&TerminalCell>, new: Option<&TerminalCell>) -> bool {
    fn fragment(cell: Option<&TerminalCell>) -> (u8, &str) {
        match cell {
            Some(cell) if cell.width == TerminalCellWidth::Continuation => (0, ""),
            Some(cell) if !cell.grapheme.is_empty() => (1, cell.grapheme.as_str()),
            _ => (2, ""),
        }
    }
    fragment(old) == fragment(new)
}

#[derive(Default)]
struct TerminalModelDiff {
    text_changed: bool,
    visual_changed: bool,
}

fn cell_paint_eq(old: Option<&TerminalCell>, new: Option<&TerminalCell>) -> bool {
    match (old, new) {
        (None, None) => true,
        (Some(old), Some(new)) => {
            old.grapheme == new.grapheme
                && old.foreground == new.foreground
                && old.background == new.background
                && old.attrs == new.attrs
                && old.width == new.width
        }
        // A missing model entry and an explicit blank are visually equivalent in
        // some cases, but treating this rare producer-side shape change as dirty
        // avoids baking those assumptions into the hot path.
        _ => false,
    }
}

fn mark_row(state: &mut TerminalGridState, row: usize, rows: usize) {
    if row < rows {
        if state.dirty_rows.len() != rows {
            state.dirty_rows.resize(rows, false);
        }
        state.dirty_rows[row] = true;
    }
}

fn mark_selection_rows(
    state: &mut TerminalGridState,
    selection: Option<TerminalSelection>,
    rows: usize,
) {
    let Some(selection) = selection else { return };
    let first = selection.start.row.min(selection.end.row);
    let last = selection
        .start
        .row
        .max(selection.end.row)
        .min(rows.saturating_sub(1));
    for row in first..=last {
        mark_row(state, row, rows);
    }
}

fn mark_cursor_row(state: &mut TerminalGridState, cursor: Option<TerminalCursor>, rows: usize) {
    if let Some(cursor) = cursor.filter(|cursor| cursor.paints()) {
        mark_row(state, cursor.position.row, rows);
    }
}

/// Diffs the model at cell granularity, but deliberately rebuilds an affected row:
/// merged background runs can extend to either neighbor. The single comparison pass
/// also decides whether the accessible plain-text value changed.
fn plan_incremental_paint(
    state: &mut TerminalGridState,
    current: &TerminalGridModel,
) -> TerminalModelDiff {
    let mut diff = TerminalModelDiff::default();
    let rows = current.rows;
    if state.model.background != current.background {
        // The backdrop is one primitive shared by every row. A palette flip needs
        // a complete rebuild; ordinary terminal traffic never takes this path.
        state.full_paint_dirty = true;
        state.dirty_rows.resize(rows, true);
        state.dirty_rows.fill(true);
        diff.visual_changed = true;
    }
    // A dynamic terminal source supplies a complete model. Comparing it once is
    // unavoidable without a patch-oriented public API, so fold text semantics into
    // the same traversal instead of allocating plain text or scanning the grid twice.
    for row in 0..rows {
        for column in 0..current.columns {
            let point = TerminalGridPoint::new(row, column);
            let old = state.model.cell(point);
            let new = current.cell(point);
            if !plain_text_cell_eq(old, new) {
                diff.text_changed = true;
            }
            if !state.full_paint_dirty && !cell_paint_eq(old, new) {
                if !state.dirty_rows.get(row).copied().unwrap_or(false) {
                    mark_row(state, row, rows);
                }
                diff.visual_changed = true;
            }
        }
    }

    if state.model.selection != current.selection {
        mark_selection_rows(state, state.model.selection, rows);
        mark_selection_rows(state, current.selection, rows);
        diff.visual_changed = true;
    }
    if state.model.cursor != current.cursor {
        mark_cursor_row(state, state.model.cursor, rows);
        mark_cursor_row(state, current.cursor, rows);
        state.cursor_dirty = true;
        diff.visual_changed = true;
    }
    if state.model.images != current.images {
        state.images_dirty = true;
        diff.visual_changed = true;
    }
    diff
}
