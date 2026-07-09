use super::*;

pub fn terminal_grid_metrics(
    runtime: &crate::widgets::Runtime,
    id: WidgetId,
) -> Option<TerminalCellMetrics> {
    runtime
        .terminal_grid
        .with(|grids| grids.borrow().get(id).map(|state| state.metrics))
}

/// Maps a window-space pointer to a mounted terminal cell and resolves its link.
pub fn terminal_grid_hit_test(
    runtime: &crate::widgets::Runtime,
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
    runtime: &crate::widgets::Runtime,
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
            clocked => {
                state.source = Some(clocked);
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
/// revision without writing a SchnellUI signal. The host wakes the window with
/// `App::redraw_signal()`, and the resulting frame reaches this poll through
/// `run_dynamic_slots`.
pub(crate) fn poll_dynamic_sources(runtime: &crate::widgets::Runtime, scene: &mut Scene) {
    let ids = runtime.terminal_grid.take_ids(|state| {
        state
            .source
            .as_ref()
            .is_some_and(DynamicTerminalGridSource::is_externally_clocked)
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
        match source {
            DynamicTerminalGridSource::Versioned {
                mut revision,
                observed_revision,
                mut model,
            } => {
                let current_revision = revision();
                if current_revision == observed_revision {
                    restore_source(
                        runtime,
                        id,
                        DynamicTerminalGridSource::Versioned {
                            revision,
                            observed_revision,
                            model,
                        },
                    );
                    continue;
                }
                let current = model();
                apply_dynamic_model(
                    runtime,
                    scene,
                    id,
                    DynamicTerminalGridSource::Versioned {
                        revision,
                        observed_revision: current_revision,
                        model,
                    },
                    current,
                );
            }
            DynamicTerminalGridSource::Patched {
                mut revision,
                observed_revision,
                mut patch,
            } => {
                let current_revision = revision();
                if current_revision != observed_revision {
                    apply_patch(runtime, scene, id, &mut patch);
                }
                restore_source(
                    runtime,
                    id,
                    DynamicTerminalGridSource::Patched {
                        revision,
                        observed_revision: current_revision,
                        patch,
                    },
                );
            }
            DynamicTerminalGridSource::Unversioned(source) => {
                // The id snapshot contains only externally clocked states.
                // Preserve this defensively if a callback caused a structural
                // mutation.
                restore_source(runtime, id, DynamicTerminalGridSource::Unversioned(source));
            }
        }
    }
    runtime.terminal_grid.return_ids(ids);
}

fn restore_source(
    runtime: &crate::widgets::Runtime,
    id: WidgetId,
    source: DynamicTerminalGridSource,
) {
    runtime.terminal_grid.with(|grids| {
        if let Some(state) = grids.borrow_mut().get_mut(id) {
            state.source = Some(source);
        }
    });
}

/// Runs a host patch against the retained state, then publishes the channels it
/// touched. The state is detached from the registry while user code runs, so no
/// registry borrow is held across the closure (SOUL §3.1). Detaching moves the
/// struct out of its slot and back; it does not allocate.
fn apply_patch(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    id: WidgetId,
    patch: &mut GridPatchFn,
) {
    let Some(mut state) = runtime
        .terminal_grid
        .with(|grids| grids.borrow_mut().remove(id))
    else {
        return;
    };
    let effects = {
        let mut handle = TerminalGridPatch::new(&mut state);
        patch(&mut handle);
        handle.effects()
    };
    if effects.resized {
        *state.measured.borrow_mut() = state
            .metrics
            .grid_size(state.model.columns, state.model.rows);
        scene.mark_dirty(id, DirtyFlags::LAYOUT);
    }
    // No PAINT mark here: `emit_terminal_grids` runs on every settled frame and
    // marks exactly what it rewrites, so the renderer can patch ranges.
    if state.a11y_full || !state.a11y_rows.is_empty() {
        sync_a11y_value(scene, id, &mut state);
    }
    runtime.terminal_grid.with(|grids| {
        grids.borrow_mut().insert(id, state);
    });
}

fn apply_dynamic_model(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    id: WidgetId,
    source: DynamicTerminalGridSource,
    current: TerminalGridModel,
) {
    let Some(mut state) = runtime
        .terminal_grid
        .with(|grids| grids.borrow_mut().remove(id))
    else {
        return;
    };
    state.source = Some(source);
    let dimensions_changed =
        state.model.columns != current.columns || state.model.rows != current.rows;
    if !dimensions_changed {
        plan_incremental_paint(&mut state, &current);
    }
    state.model = current;
    if dimensions_changed {
        state.mark_all_dirty();
        *state.measured.borrow_mut() = state
            .metrics
            .grid_size(state.model.columns, state.model.rows);
        scene.mark_dirty(id, DirtyFlags::LAYOUT);
    }
    // Paint is marked by `emit_terminal_grids`, which runs every settled frame.
    if state.a11y_full || !state.a11y_rows.is_empty() {
        sync_a11y_value(scene, id, &mut state);
    }
    runtime.terminal_grid.with(|grids| {
        grids.borrow_mut().insert(id, state);
    });
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

pub(super) fn mark_selection_rows(
    state: &mut TerminalGridState,
    selection: Option<TerminalSelection>,
) {
    let Some(selection) = selection else { return };
    let rows = state.model.rows;
    let first = selection.start.row.min(selection.end.row);
    let last = selection
        .start
        .row
        .max(selection.end.row)
        .min(rows.saturating_sub(1));
    for row in first..=last {
        state.dirty_rows.insert(row);
    }
}

pub(super) fn mark_cursor_row(state: &mut TerminalGridState, cursor: Option<TerminalCursor>) {
    if let Some(cursor) = cursor.filter(|cursor| cursor.paints()) {
        state.dirty_rows.insert(cursor.position.row);
    }
}

/// Diffs a complete host model at cell granularity, but deliberately rebuilds an
/// affected row: merged background runs can extend to either neighbor. The same
/// pass records rows whose accessible text changed. `TerminalGrid::patched` skips
/// this diff entirely.
fn plan_incremental_paint(state: &mut TerminalGridState, current: &TerminalGridModel) {
    if state.model.background != current.background {
        // The backdrop is one primitive shared by every row. A palette flip needs
        // a complete rebuild; ordinary terminal traffic never takes this path.
        state.full_paint_dirty = true;
        state.dirty_rows.reset_all(current.rows);
    }
    for row in 0..current.rows {
        for column in 0..current.columns {
            let point = TerminalGridPoint::new(row, column);
            let old = state.model.cell(point);
            let new = current.cell(point);
            if !plain_text_cell_eq(old, new) {
                state.a11y_rows.insert(row);
            }
            if !state.full_paint_dirty && !cell_paint_eq(old, new) {
                state.dirty_rows.insert(row);
            }
        }
    }

    if state.model.selection != current.selection {
        mark_selection_rows(state, state.model.selection);
        mark_selection_rows(state, current.selection);
    }
    if state.model.cursor != current.cursor {
        mark_cursor_row(state, state.model.cursor);
        mark_cursor_row(state, current.cursor);
        state.cursor_dirty = true;
    }
    if state.model.images != current.images {
        state.images_dirty = true;
    }
}
