//! Hooks for property-based fuzzing of real input sequences, behind the
//! `fuzzing` feature. The `schnellui-fuzz` workspace crate builds its
//! properties on these.
//!
//! [`HeadlessHost`] replays pointer, key, wheel, resize and theme input through
//! the windowed host's own handlers. The checks report retained state that
//! went wrong in a way pixels alone hide: a decoration whose record no longer
//! matches the paint it sits on, a decoration sized for paint that has since
//! moved, or a tooltip or floating layer that left the window.

pub use crate::windowed::headless::{
    ClipboardAction, HeadlessHost, HostKey, PointerButton, PointerRoute,
};
/// How soon after a press the next press on the same text field still counts
/// as the next click of a double or triple click.
pub const MULTI_CLICK_INTERVAL: std::time::Duration = crate::windowed::MULTI_CLICK_INTERVAL;
/// How far from a press, on either axis and in logical pixels, the next
/// press may land and still count with it.
pub const MULTI_CLICK_SLOP: f32 = crate::windowed::MULTI_CLICK_SLOP;

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

/// The hover decoration is on the control under the resting `pointer` (or on
/// nothing while the keyboard holds hover). Describes the mismatch.
pub fn hover_faults(app: &App, pointer: schnellui::scene::Point) -> Option<String> {
    crate::widgets::probes::hover_disagrees(
        &app.widgets,
        app.scene(),
        pointer,
        app.hover_hold.get().is_some(),
    )
}

/// A scroll viewport that draws and takes a native scrollbar right now.
#[derive(Clone, Copy, Debug)]
pub struct ScrollbarTarget {
    pub viewport: schnellui::scene::WidgetId,
    pub track: schnellui::scene::Rect,
    pub thumb: schnellui::scene::Rect,
}

/// Every viewport built with `.scrollbar(true)` whose content overflows, in
/// tree order, with the window-space track and thumb the pointer can grab.
pub fn scrollbars(app: &App) -> Vec<ScrollbarTarget> {
    let scene = app.scene();
    let mut out = Vec::new();
    for id in scene.preorder() {
        if !crate::widgets::scroll_has_scrollbar(&app.widgets, id) || !scene.is_visible(id) {
            continue;
        }
        let Some((track, thumb)) =
            crate::widgets::scroll_metrics(scene, id).and_then(crate::widgets::scrollbar_rects)
        else {
            continue;
        };
        out.push(ScrollbarTarget {
            viewport: id,
            track,
            thumb,
        });
    }
    out
}

/// Every visible text, image, chart or control that a scrollbar covers: its
/// box, or for text its glyphs, reaches into the bar's track while some of it
/// shows in the viewport. Layout keeps a gutter for the bar, so this lists
/// only content the gutter cannot save, such as a child wider than its viewport.
pub fn scrollbar_overlaps(app: &App) -> Vec<String> {
    use schnellui::scene::WidgetKind;
    let scene = app.scene();
    let mut found = Vec::new();
    for bar in scrollbars(app) {
        let mut stack = vec![bar.viewport];
        while let Some(id) = stack.pop() {
            let Some(node) = scene.node(id) else { continue };
            stack.extend(node.children.iter().copied());
            if id == bar.viewport || !scene.is_visible(id) {
                continue;
            }
            let text = matches!(node.kind, WidgetKind::Text | WidgetKind::RichText);
            let picture = matches!(node.kind, WidgetKind::Image | WidgetKind::Chart);
            let control = scene.a11y(id).is_some_and(|a| {
                use schnellui::a11y::Role;
                matches!(
                    Role::from_u16(a.role),
                    Role::Button
                        | Role::Tab
                        | Role::ProgressIndicator
                        | Role::Table
                        | Role::CheckBox
                        | Role::TextInput
                        | Role::Slider
                        | Role::Switch
                )
            });
            let Some(layout) = scene.layout(id) else {
                continue;
            };
            if !(text || picture || control) || layout.rect.is_empty() {
                continue;
            }
            let (offset, clip) = scene.composite_context(id);
            let (top, bottom) = (layout.rect.y + offset.y, layout.rect.bottom() + offset.y);
            if bottom <= clip.y.max(bar.track.y) || top >= clip.bottom().min(bar.track.bottom()) {
                continue;
            }
            let mut right = layout.rect.right();
            if text {
                if let Some(paint) = scene.paint(id) {
                    for primitive in &paint.primitives {
                        if let schnellui::scene::Primitive::GlyphQuad { rect, .. } = primitive {
                            right = right.max(rect.right());
                        }
                    }
                }
            }
            if right > bar.track.x + 0.01 && layout.rect.x < bar.track.right() {
                let name = scene
                    .a11y(id)
                    .map(|a| format!("{:?} {:?}", a.role, a.name))
                    .unwrap_or_default();
                found.push(format!(
                    "{:?} {name} ends at x {right:.2}, past the scrollbar track starting at {:.2}",
                    node.kind, bar.track.x
                ));
            }
        }
    }
    found
}

/// The layout-space rect a keyboard reveal aims at for the control `id`: the
/// caret row of a text area, else the control's own box with its focus ring.
/// `None` while the control has no layout, or an area has no row table yet.
pub fn reveal_target(
    app: &mut App,
    id: schnellui::scene::WidgetId,
) -> Option<schnellui::scene::Rect> {
    let widgets = app.widgets.clone();
    crate::widgets::reveal_target(&widgets, &app.scene, &mut app.text, id)
}

/// Whether the control `id`, or for a focused text area its caret row, shows
/// inside the window: its [`reveal_target`] moved into window space and cut by
/// every enclosing viewport leaves something to see.
pub fn visible_in_window(app: &mut App, id: schnellui::scene::WidgetId) -> bool {
    let Some(target) = reveal_target(app, id) else {
        return false;
    };
    let (offset, clip) = app.scene().composite_context(id);
    let size = app.size();
    let window = schnellui::scene::Rect::new(0.0, 0.0, size.width, size.height);
    let shown = schnellui::scene::Rect::new(
        target.x + offset.x,
        target.y + offset.y,
        target.width,
        target.height,
    )
    .intersect(&clip)
    .intersect(&window);
    !shown.is_empty()
}

/// Tabs once around the whole tab order and reports the first stop that stays
/// out of the window with nothing to scroll it in. `Ok` carries the number of
/// stops visited.
pub fn tab_stops_reachable(app: &mut App) -> Result<usize, String> {
    use crate::UiKey;
    app.frame();
    let mut first = None;
    for visited in 0..512 {
        app.dispatch_key(UiKey::Tab { shift: false });
        app.frame();
        let Some(id) = app.focused_widget() else {
            return Ok(visited);
        };
        if first == Some(id) {
            return Ok(visited);
        }
        first.get_or_insert(id);
        if !visible_in_window(app, id) {
            let name = app
                .scene()
                .a11y(id)
                .map(|a| format!("{:?} {:?}", a.role, a.name))
                .unwrap_or_default();
            let rect = app.scene().layout(id).map(|l| l.rect);
            return Err(format!(
                "tab stop {visited} ({name}, layout {rect:?}) is outside the {:?} window",
                app.size()
            ));
        }
    }
    Err("the tab order does not wrap within 512 stops".into())
}

/// Every visible text node, image, chart or a11y control that the window cuts
/// off at a side, in tree order, described one per entry. A vertical cut is
/// ignored: a page scrolls down.
///
/// A Scroll viewport scrolls only down, so any content its viewport clips at a
/// side is unreachable, and so is anything past the window edge. Content scrolled
/// wholly out of view is skipped: it is not cut, it is below the fold.
pub fn sideways_clips(app: &mut App) -> Vec<String> {
    use schnellui::scene::WidgetKind;
    let mut found = Vec::new();
    app.frame();
    let size = app.size();
    let scene = app.scene();
    for id in scene.preorder() {
        if !scene.is_visible(id) {
            continue;
        }
        let Some(node) = scene.node(id) else { continue };
        let text = matches!(node.kind, WidgetKind::Text | WidgetKind::RichText);
        let picture = matches!(node.kind, WidgetKind::Image | WidgetKind::Chart);
        let control = scene.a11y(id).is_some_and(|a| {
            use schnellui::a11y::Role;
            matches!(
                Role::from_u16(a.role),
                Role::Button
                    | Role::Tab
                    | Role::ProgressIndicator
                    | Role::Table
                    | Role::CheckBox
                    | Role::TextInput
                    | Role::Slider
                    | Role::Switch
            )
        });
        if !(text || control || picture) {
            continue;
        }
        let Some(layout) = scene.layout(id) else {
            continue;
        };
        if layout.rect.is_empty() {
            continue;
        }
        let (offset, clip) = scene.composite_context(id);
        // Scrolled wholly out of view, so a side cannot cut it.
        let (top, bottom) = (layout.rect.y + offset.y, layout.rect.bottom() + offset.y);
        if clip.height <= 0.0 || bottom <= clip.y || top >= clip.bottom() {
            continue;
        }
        let left = layout.rect.x + offset.x;
        let mut right = left + layout.rect.width;
        if text {
            // A line that does not wrap runs past its box: the glyphs, not the
            // box, say how far.
            if let Some(paint) = scene.paint(id) {
                for primitive in &paint.primitives {
                    if let schnellui::scene::Primitive::GlyphQuad { rect, .. } = primitive {
                        right = right.max(rect.right() + offset.x);
                    }
                }
            }
        }
        let shown_left = clip.x.max(0.0);
        let shown_right = clip.right().min(size.width);
        if left < shown_left - 0.5 || right > shown_right + 0.5 {
            let name = scene
                .a11y(id)
                .map(|a| format!("{:?} {:?}", a.role, a.name))
                .unwrap_or_default();
            found.push(format!(
                "{:?} {name} spans x {left:.0}..{right:.0} but only {shown_left:.0}..{shown_right:.0} shows in the {:?} window",
                node.kind,
                app.size()
            ));
        }
    }
    found
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

/// Where two apps differ in what a user or an assistive client can see: the
/// size, then each node's role, name, value and layout rect in tree order.
/// `None` when they match. For tests that mount at one size, resize live and
/// compare against a fresh mount at the new size.
pub fn layout_diff(live: &mut App, fresh: &mut App) -> Option<String> {
    live.frame();
    fresh.frame();
    if live.size() != fresh.size() {
        return Some(format!(
            "size {:?} against a fresh {:?}",
            live.size(),
            fresh.size()
        ));
    }
    let (a, b) = (live.scene(), fresh.scene());
    let (ids_a, ids_b): (Vec<_>, Vec<_>) = (a.preorder().collect(), b.preorder().collect());
    for (index, (&x, &y)) in ids_a.iter().zip(&ids_b).enumerate() {
        let semantics = |scene: &schnellui::scene::Scene, id| {
            scene
                .a11y(id)
                .map(|s| (s.role, s.name.clone(), s.value.clone()))
        };
        if semantics(a, x) != semantics(b, y) {
            return Some(format!(
                "node {index}: {:?} against a fresh {:?}",
                semantics(a, x),
                semantics(b, y)
            ));
        }
        let rect = |scene: &schnellui::scene::Scene, id| scene.layout(id).map(|l| l.rect);
        let same = match (rect(a, x), rect(b, y)) {
            (Some(p), Some(q)) => {
                (p.x - q.x).abs() < 0.01
                    && (p.y - q.y).abs() < 0.01
                    && (p.width - q.width).abs() < 0.01
                    && (p.height - q.height).abs() < 0.01
            }
            (p, q) => p.is_none() == q.is_none(),
        };
        if !same {
            return Some(format!(
                "node {index} {:?}: rect {:?} against a fresh {:?}",
                semantics(a, x),
                rect(a, x),
                rect(b, y)
            ));
        }
    }
    (ids_a.len() != ids_b.len())
        .then(|| format!("{} nodes against a fresh {}", ids_a.len(), ids_b.len()))
}

/// Mounts each scenario at `launch`, resizes the live window to `resized`
/// through the window host, and checks it against a fresh mount at `resized`
/// ([`layout_diff`]), at every scale. `mount` builds a scenario at a logical
/// size and scale, as an example's `scenario_app` does. Panics on a mismatch.
pub fn assert_resize_matches_fresh<S: Copy + std::fmt::Debug>(
    scenarios: impl IntoIterator<Item = S>,
    launch: (u32, u32),
    resized: (u32, u32),
    scales: &[f32],
    mount: impl Fn(S, u32, u32, f32) -> App,
) {
    let scenarios: Vec<S> = scenarios.into_iter().collect();
    for &scale in scales {
        for &scenario in &scenarios {
            let mut app = mount(scenario, launch.0, launch.1, scale);
            app.frame();
            let mut host = HeadlessHost::new(app);
            host.resize(resized.0 as f32, resized.1 as f32);
            let mut fresh = mount(scenario, resized.0, resized.1, scale);
            assert_eq!(
                layout_diff(host.app_mut(), &mut fresh),
                None,
                "{scenario:?} at scale {scale}: {launch:?} resized to {resized:?}"
            );
        }
    }
}
