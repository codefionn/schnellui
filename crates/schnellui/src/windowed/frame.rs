//! Moving and resizing a window opened without the system frame
//! ([`App::set_window_decorations`]). The app draws its own title bar; a
//! press on its empty part ([`App::set_window_move_area`]) or on the thin band
//! along the window's edges hands the move or the resize to the window system.

use winit::window::ResizeDirection;

use super::*;

/// Width of the band along each edge that resizes the window, in logical px.
const RESIZE_BORDER: f32 = 4.0;
/// How far a corner reaches along the edges on either side of it, so a corner
/// is easier to grab than the thin band.
const RESIZE_CORNER: f32 = 12.0;

/// The host's memory of the frame it stands in for.
#[derive(Default)]
pub(super) struct FrameState {
    /// When the last press in the move area handed a move to the window
    /// system, so a second one soon after maximizes instead.
    last_move_press: Option<Instant>,
    /// Whether the window is maximized, as last seen on a resize.
    pub(super) maximized: bool,
    /// Whether the window is full screen, as last seen on a resize.
    fullscreen: bool,
}

/// Which edge or corner of a `size` window the logical point `p` grabs, if any.
pub(super) fn resize_direction(p: Point, size: Size) -> Option<ResizeDirection> {
    let inside = p.x >= 0.0 && p.y >= 0.0 && p.x < size.width && p.y < size.height;
    let on_band = p.x < RESIZE_BORDER
        || p.y < RESIZE_BORDER
        || p.x >= size.width - RESIZE_BORDER
        || p.y >= size.height - RESIZE_BORDER;
    if !inside || !on_band {
        return None;
    }
    let west = p.x < RESIZE_CORNER;
    let east = p.x >= size.width - RESIZE_CORNER;
    let north = p.y < RESIZE_CORNER;
    let south = p.y >= size.height - RESIZE_CORNER;
    Some(match (north, south, west, east) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (true, _, _, true) => ResizeDirection::NorthEast,
        (_, true, true, _) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, ..) => ResizeDirection::North,
        (_, true, ..) => ResizeDirection::South,
        (_, _, true, _) => ResizeDirection::West,
        _ => ResizeDirection::East,
    })
}

pub(super) fn resize_cursor(direction: ResizeDirection) -> UiCursorIcon {
    match direction {
        ResizeDirection::North => UiCursorIcon::NResize,
        ResizeDirection::South => UiCursorIcon::SResize,
        ResizeDirection::East => UiCursorIcon::EResize,
        ResizeDirection::West => UiCursorIcon::WResize,
        ResizeDirection::NorthEast => UiCursorIcon::NeResize,
        ResizeDirection::NorthWest => UiCursorIcon::NwResize,
        ResizeDirection::SouthEast => UiCursorIcon::SeResize,
        ResizeDirection::SouthWest => UiCursorIcon::SwResize,
    }
}

/// Whether a press on `id` may move the window: it paints, but takes no
/// input, and no overlay such as a dialog or a menu holds it.
fn passive(scene: &schnellui::scene::Scene, id: SceneWidgetId) -> bool {
    let kind = scene.node(id).map(|node| node.kind);
    if !matches!(
        kind,
        Some(
            WidgetKind::Text
                | WidgetKind::Image
                | WidgetKind::Icon
                | WidgetKind::Divider
                | WidgetKind::Badge
                | WidgetKind::ProgressBar
                | WidgetKind::LoadingSpinner
        )
    ) {
        return false;
    }
    let mut current = Some(id);
    while let Some(node) = current {
        if scene.is_overlay(node) {
            return false;
        }
        current = scene.node(node).and_then(|n| n.parent);
    }
    true
}

impl WindowedApp {
    /// Follows a request from the app's own title bar.
    pub(super) fn apply_window_request(
        &mut self,
        event_loop: &ActiveEventLoop,
        request: WindowRequest,
    ) {
        let Some(window) = &self.window else {
            return;
        };
        match request {
            WindowRequest::Minimize => window.set_minimized(true),
            WindowRequest::ToggleMaximize => window.set_maximized(!window.is_maximized()),
            WindowRequest::Close => event_loop.exit(),
        }
    }

    /// Records whether the window is maximized or full screen now, for the
    /// app's restore button and for the resize band, which neither has.
    pub(super) fn note_window_maximized(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let maximized = window.is_maximized();
        self.frame.maximized = maximized;
        self.frame.fullscreen = window.fullscreen().is_some();
        self.app.redraw_signal.note_maximized(maximized);
    }

    /// The edge or corner a press at `p` would resize. Only a window without
    /// the system frame that is neither maximized nor full screen has the
    /// band. On macOS the system keeps resizing frameless windows itself.
    pub(super) fn resize_edge_at(&self, p: Point) -> Option<ResizeDirection> {
        // Cached state only: this runs on every pointer move.
        if cfg!(target_os = "macos")
            || self.decorated
            || self.window.is_none()
            || self.frame.maximized
            || self.frame.fullscreen
        {
            return None;
        }
        resize_direction(p, self.app.size())
    }

    /// Hands a primary press at `p` to the window system when it lands on the
    /// resize band or the app's move area. Returns `true` when it did, and the
    /// press then reaches no widget.
    pub(super) fn begin_frame_drag(&mut self, p: Point) -> bool {
        if self.decorated {
            return false;
        }
        let Some(window) = self.window.clone() else {
            return false;
        };
        if let Some(direction) = self.resize_edge_at(p) {
            return window.drag_resize_window(direction).is_ok();
        }
        if !self.app.in_window_move_area(p) {
            return false;
        }
        let hit = schnellui::widgets::hit_test(&self.app.widgets.clone(), self.app.scene(), p);
        if hit.is_some_and(|id| !passive(self.app.scene(), id)) {
            return false;
        }
        // The press lands outside any open menu, which closes as it would on
        // a press anywhere else.
        if self.app.dismiss_context_menu() {
            window.request_redraw();
        }
        let now = self.app.clock_now();
        if self
            .frame
            .last_move_press
            .take()
            .is_some_and(|at| now.duration_since(at) <= MULTI_CLICK_INTERVAL)
        {
            window.set_maximized(!window.is_maximized());
            return true;
        }
        self.frame.last_move_press = Some(now);
        window.drag_window().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Size = Size {
        width: 800.0,
        height: 600.0,
    };

    fn at(x: f32, y: f32) -> Option<ResizeDirection> {
        resize_direction(Point { x, y }, SIZE)
    }

    #[test]
    fn the_band_along_each_edge_resizes_that_edge() {
        assert_eq!(at(400.0, 0.0), Some(ResizeDirection::North));
        assert_eq!(at(400.0, 599.0), Some(ResizeDirection::South));
        assert_eq!(at(0.0, 300.0), Some(ResizeDirection::West));
        assert_eq!(at(799.5, 300.0), Some(ResizeDirection::East));
    }

    #[test]
    fn corners_reach_along_the_edges() {
        assert_eq!(at(10.0, 1.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(1.0, 10.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(795.0, 2.0), Some(ResizeDirection::NorthEast));
        assert_eq!(at(2.0, 595.0), Some(ResizeDirection::SouthWest));
        assert_eq!(at(799.0, 590.0), Some(ResizeDirection::SouthEast));
    }

    #[test]
    fn the_inside_and_the_outside_do_not_resize() {
        assert_eq!(at(400.0, 300.0), None);
        assert_eq!(at(10.0, 10.0), None);
        assert_eq!(at(-1.0, 300.0), None);
        assert_eq!(at(f32::NEG_INFINITY, f32::NEG_INFINITY), None);
        assert_eq!(at(800.0, 300.0), None);
    }
}
