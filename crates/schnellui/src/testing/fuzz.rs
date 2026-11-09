//! Hooks for property-based fuzzing of real input sequences, behind the
//! `fuzzing` feature. The `schnellui-fuzz` workspace crate builds its
//! properties on these.
//!
//! [`HeadlessHost`] replays pointer, key, wheel, resize and theme input through
//! the windowed host's own handlers. The checks report retained state that
//! went wrong in a way pixels alone hide: a decoration whose record no longer
//! matches the paint it sits on, a decoration sized for paint that has since
//! moved, or a tooltip or floating layer that left the window.

pub use crate::windowed::headless::{HeadlessHost, HostKey, PointerButton};

use schnellui::widgets::{ComponentInteractions, InteractionStates, InteractionStyle, Theme};
use schnellui::App;

/// Every applied hover, press and focus decoration still finds the
/// primitives it recorded.
pub fn decoration_faults(app: &App) -> Option<String> {
    crate::widgets::probes::decoration_faults(&app.widgets, app.scene())
}

/// Takes every decoration off and lays it again on the same targets, and
/// describes the first node whose geometry changed. A difference means a
/// decoration kept the size or place of paint that has since moved.
pub fn decoration_relay_diff(app: &mut App) -> Option<String> {
    let widgets = app.widgets.clone();
    crate::widgets::probes::decoration_relay_diff(&widgets, app.scene_mut())
}

/// A shown tooltip, dialog, dropdown popup or context menu that fits inside
/// the window but sits partly outside it.
pub fn placement_faults(app: &App) -> Option<String> {
    let size = app.size();
    let window = schnellui::scene::Rect::new(0.0, 0.0, size.width, size.height);
    crate::widgets::probes::placement_faults(&app.widgets, app.scene(), window)
}

/// All of the checks above, first failure first.
pub fn invariants(app: &mut App) -> Result<(), String> {
    if let Some(fault) = decoration_faults(app) {
        return Err(format!("decoration record: {fault}"));
    }
    if let Some(fault) = decoration_relay_diff(app) {
        return Err(format!("stale decoration: {fault}"));
    }
    if let Some(fault) = placement_faults(app) {
        return Err(format!("placement: {fault}"));
    }
    Ok(())
}

/// `theme` with every hover, press and focus decoration removed. A host on
/// this theme paints what the decorated host must paint once the pointer is
/// gone, the button is up and nothing has focus.
pub fn strip_interactions(theme: Theme) -> Theme {
    Theme {
        interactions: InteractionStates {
            hover: InteractionStyle::NONE,
            focus: InteractionStyle::NONE,
            active: InteractionStyle::NONE,
        },
        component_interactions: ComponentInteractions::NONE,
        ..theme
    }
}
