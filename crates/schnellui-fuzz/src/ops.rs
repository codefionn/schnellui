//! Input a person can produce, as proptest strategies, and how a host
//! replays it.

use std::time::Duration;

use proptest::prelude::*;
use schnellui::accesskit_action::{Action, ActionRequest};
use schnellui::accesskit_reexport::TreeId;
use schnellui::scene::{Point, WidgetId, WidgetKind};
use schnellui::testing::fuzz::scrollbars;
use schnellui::App;

use crate::{ClipboardAction, HeadlessHost, HostKey, PointerButton, THEMES};

/// One input event. Pointer targets are resolved when the op runs, against
/// whatever is on screen then, so a sequence stays meaningful after layout
/// changes and shrinks well.
#[derive(Clone, Debug)]
pub enum Op {
    /// Point at the `n`th actionable widget on screen (wrapping), at its
    /// centre or just inside its top-left corner.
    Aim {
        n: u8,
        corner: bool,
    },
    /// Point anywhere, as fractions of the window.
    Wander(f32, f32),
    Press(PointerButton),
    Release(PointerButton),
    /// A fast click: point at the `n`th actionable widget as `Aim` does, then
    /// press and release `count` times with no frame between any of it. Two
    /// is a double click, three a triple click.
    Click {
        n: u8,
        corner: bool,
        button: PointerButton,
        count: u8,
    },
    /// The same fast click at a fixed point, as fractions of the window. What
    /// [`pinned`] turns a `Click` into, so that several hosts get one point.
    ClickAt {
        x: f32,
        y: f32,
        button: PointerButton,
        count: u8,
    },
    /// A left click on the `n`th actionable widget as the debug server's
    /// `click-at` delivers it: move, press, release, a frame after each.
    TapOn {
        n: u8,
        corner: bool,
    },
    /// The same click at a fixed point, as fractions of the window.
    Tap(f32, f32),
    /// Move to a point of the window and press there, as the debug server's
    /// `press` with a point does.
    PressAt(f32, f32, PointerButton),
    /// Move to a point of the window and release there.
    ReleaseAt(f32, f32, PointerButton),
    /// A whole drag as the debug server's `drag` delivers it: press on the
    /// `n`th actionable widget, move to a point of the window in `steps`
    /// equal steps, release.
    DragFrom {
        n: u8,
        corner: bool,
        to: (f32, f32),
        steps: u8,
        button: PointerButton,
    },
    /// The same drag between two fixed points.
    Drag {
        from: (f32, f32),
        to: (f32, f32),
        steps: u8,
        button: PointerButton,
    },
    Leave,
    Key(HostKey, bool),
    /// A key with Ctrl held (Ctrl+Home, Ctrl+End, Ctrl+arrows).
    Chord(HostKey),
    /// One wheel event at the pointer, in logical pixels, positive scrolling
    /// down. Deltas run from trackpad steps of a pixel or two to a flung
    /// notch.
    Wheel(f32),
    /// `count` wheel events of the same delta in a row, as a flick or a held
    /// notch delivers them.
    WheelBurst(f32, u8),
    /// Move to a point of the window, as fractions of it, then wheel there:
    /// scrolling a viewport the pointer was not resting on.
    WheelAt(f32, f32, f32),
    /// Point at the thumb or track of the `n`th native scrollbar on screen
    /// (wrapping). Press, Wander and Release then drag or page it.
    AimBar {
        n: u8,
        part: BarPart,
    },
    /// A whole drag of the `n`th scrollbar's thumb: press on it, move in three
    /// steps until the thumb's top sits at `to` of its travel while the pointer
    /// drifts `drift` pixels off the track sideways, release.
    DragBar {
        n: u8,
        to: f32,
        drift: f32,
    },
    /// An AccessKit scroll action on the `n`th node on screen (wrapping), or
    /// on the `n`th scroll viewport when `viewport` is set.
    Access {
        n: u8,
        viewport: bool,
        action: ScrollAction,
    },
    /// Resize to fractions of the mounted size.
    Resize(f32, f32),
    Blur,
    Theme(usize),
    /// Let this many milliseconds of virtual time pass, then draw a frame.
    Wait(u32),
    /// Start (`true`) or end a burst: events inside one are handled without
    /// a frame between them, as input faster than the display is.
    Burst(bool),
    /// Draw a frame now, even inside a burst.
    Frame,
    /// Fade to a theme over this many milliseconds.
    ThemeFade(usize, u32),
    /// Copy, cut or paste, as the clipboard shortcuts do.
    Clipboard(ClipboardAction),
}

/// Where on a scrollbar a pointer is aimed.
#[derive(Clone, Copy, Debug)]
pub enum BarPart {
    Thumb,
    /// The track between its top and the thumb.
    Above,
    /// The track between the thumb and its bottom.
    Below,
}

/// The AccessKit scroll actions.
#[derive(Clone, Copy, Debug)]
pub enum ScrollAction {
    Up,
    Down,
    IntoView,
}

impl ScrollAction {
    fn action(self) -> Action {
        match self {
            ScrollAction::Up => Action::ScrollUp,
            ScrollAction::Down => Action::ScrollDown,
            ScrollAction::IntoView => Action::ScrollIntoView,
        }
    }
}

/// A wheel delta a person can produce: a trackpad step, a notch of one or
/// more lines, or a hard fling.
fn wheel_delta() -> impl Strategy<Value = f32> {
    let magnitude = prop_oneof![
        3 => 1.0f32..12.0,
        3 => prop_oneof![Just(48.0f32), Just(96.0), Just(144.0)],
        1 => 200.0f32..2000.0,
    ];
    (magnitude, any::<bool>()).prop_map(|(m, up)| if up { -m } else { m })
}

fn scroll_key() -> impl Strategy<Value = HostKey> {
    prop_oneof![
        Just(HostKey::PageUp),
        Just(HostKey::PageDown),
        Just(HostKey::Home),
        Just(HostKey::End),
        Just(HostKey::Up),
        Just(HostKey::Down),
        Just(HostKey::Space),
        Just(HostKey::Tab),
    ]
}

/// Input that scrolls, at fixed points of the window or by viewport number:
/// nothing in it depends on where the previous input moved the content, so
/// pace properties can replay it.
fn scroll_input_fixed() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => wheel_delta().prop_map(Op::Wheel),
        1 => (wheel_delta(), 2u8..12).prop_map(|(d, n)| Op::WheelBurst(d, n)),
        2 => (0.0f32..1.0, 0.0f32..1.0, wheel_delta()).prop_map(|(x, y, d)| Op::WheelAt(x, y, d)),
        3 => (scroll_key(), proptest::bool::weighted(0.2)).prop_map(|(k, shift)| Op::Key(k, shift)),
        1 => scroll_key().prop_map(Op::Chord),
        2 => (any::<u8>(), prop_oneof![Just(ScrollAction::Up), Just(ScrollAction::Down), Just(ScrollAction::IntoView)])
            .prop_map(|(n, action)| Op::Access { n, viewport: true, action }),
    ]
}

/// Input that scrolls, including targets chosen from what is on screen when
/// it runs: scrollbars and any node.
fn scroll_input() -> impl Strategy<Value = Op> {
    let part = prop_oneof![
        Just(BarPart::Thumb),
        Just(BarPart::Above),
        Just(BarPart::Below)
    ];
    prop_oneof![
        6 => scroll_input_fixed(),
        2 => (any::<u8>(), part).prop_map(|(n, part)| Op::AimBar { n, part }),
        2 => (any::<u8>(), 0.0f32..1.0, -80.0f32..80.0).prop_map(|(n, to, drift)| Op::DragBar { n, to, drift }),
        2 => (any::<u8>(), prop_oneof![Just(ScrollAction::Up), Just(ScrollAction::Down), Just(ScrollAction::IntoView)])
            .prop_map(|(n, action)| Op::Access { n, viewport: false, action }),
    ]
}

fn clipboard() -> impl Strategy<Value = Op> {
    prop_oneof![
        Just(ClipboardAction::Copy),
        Just(ClipboardAction::Cut),
        Just(ClipboardAction::Paste),
    ]
    .prop_map(Op::Clipboard)
}

fn key() -> impl Strategy<Value = HostKey> {
    prop_oneof![
        Just(HostKey::Tab),
        Just(HostKey::Enter),
        Just(HostKey::Space),
        Just(HostKey::Backspace),
        Just(HostKey::Delete),
        Just(HostKey::Left),
        Just(HostKey::Right),
        Just(HostKey::Up),
        Just(HostKey::Down),
        Just(HostKey::Home),
        Just(HostKey::End),
        Just(HostKey::PageUp),
        Just(HostKey::PageDown),
        Just(HostKey::Escape),
        Just(HostKey::SelectAll),
        // Typed text as the window host delivers it: it never passes a
        // control character on as text.
        prop_oneof![Just("x"), Just("W"), Just(" "), Just("é")]
            .prop_map(|text| HostKey::Text(text.to_owned())),
    ]
}

/// A fast click on a widget: mostly the left button, once, twice or three
/// times.
fn click() -> impl Strategy<Value = Op> {
    let button = prop_oneof![6 => Just(PointerButton::Left), 1 => Just(PointerButton::Right)];
    let count = prop_oneof![3 => Just(1u8), 2 => Just(2u8), 1 => Just(3u8)];
    (any::<u8>(), any::<bool>(), button, count).prop_map(|(n, corner, button, count)| Op::Click {
        n,
        corner,
        button,
        count,
    })
}

/// Any single op, weighted towards pointing and pressing.
pub fn op() -> impl Strategy<Value = Op> {
    let button = prop_oneof![4 => Just(PointerButton::Left), 1 => Just(PointerButton::Right)];
    prop_oneof![
        4 => click(),
        8 => (any::<u8>(), any::<bool>()).prop_map(|(n, corner)| Op::Aim { n, corner }),
        2 => (0.0f32..1.0, 0.0f32..1.0).prop_map(|(x, y)| Op::Wander(x, y)),
        4 => button.clone().prop_map(Op::Press),
        4 => button.prop_map(Op::Release),
        1 => Just(Op::Leave),
        6 => (key(), proptest::bool::weighted(0.2)).prop_map(|(k, shift)| Op::Key(k, shift)),
        6 => scroll_input(),
        1 => (0.2f32..1.3, 0.2f32..1.3).prop_map(|(w, h)| Op::Resize(w, h)),
        1 => Just(Op::Blur),
        1 => (0..THEMES.len()).prop_map(Op::Theme),
        1 => clipboard(),
    ]
}

/// Any op, or one that changes the pace: waits short and long, bursts of
/// input with no frame between events, forced frames and theme fades.
pub fn timed_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        6 => op(),
        1 => prop_oneof![0u32..20, 20u32..250, 250u32..1500].prop_map(Op::Wait),
        1 => any::<bool>().prop_map(Op::Burst),
        1 => Just(Op::Frame),
        1 => (0..THEMES.len(), prop_oneof![Just(0u32), 50u32..600]).prop_map(|(i, ms)| Op::ThemeFade(i, ms)),
    ]
}

/// A sequence of up to `max` timed ops.
pub fn timed_ops(max: usize) -> impl Strategy<Value = Vec<Op>> {
    proptest::collection::vec(timed_op(), 1..max)
}

/// Keyboard and pointer input for the pace properties, which replay it at
/// different speeds: keys, typing, pointing at fixed points of the window,
/// and fast clicks on widgets. A click is aimed once, by the slowest replay,
/// and [`pinned`] to that point for the others.
pub fn pace_op() -> impl Strategy<Value = Op> {
    let button = prop_oneof![4 => Just(PointerButton::Left), 1 => Just(PointerButton::Right)];
    prop_oneof![
        3 => click(),
        3 => (0.0f32..1.0, 0.0f32..1.0).prop_map(|(x, y)| Op::Wander(x, y)),
        2 => button.clone().prop_map(Op::Press),
        2 => button.prop_map(Op::Release),
        1 => Just(Op::Leave),
        6 => (key(), proptest::bool::weighted(0.2)).prop_map(|(k, shift)| Op::Key(k, shift)),
        3 => scroll_input_fixed(),
        1 => Just(Op::Blur),
        1 => clipboard(),
    ]
}

/// Input for the route property, which replays it through the window host's
/// handlers and through the debug server's commands: everything the debug
/// server has a pointer command for, with keys, wheel and waits between.
/// Every event gets its own frame, since a debug command always draws one.
pub fn route_op() -> impl Strategy<Value = Op> {
    let button = || prop_oneof![4 => Just(PointerButton::Left), 1 => Just(PointerButton::Right)];
    let spot = || (0.0f32..1.0, 0.0f32..1.0);
    prop_oneof![
        3 => (any::<u8>(), any::<bool>()).prop_map(|(n, corner)| Op::Aim { n, corner }),
        2 => spot().prop_map(|(x, y)| Op::Wander(x, y)),
        2 => button().prop_map(Op::Press),
        2 => button().prop_map(Op::Release),
        1 => (spot(), button()).prop_map(|((x, y), b)| Op::PressAt(x, y, b)),
        1 => (spot(), button()).prop_map(|((x, y), b)| Op::ReleaseAt(x, y, b)),
        5 => (any::<u8>(), any::<bool>()).prop_map(|(n, corner)| Op::TapOn { n, corner }),
        2 => spot().prop_map(|(x, y)| Op::Tap(x, y)),
        3 => (any::<u8>(), any::<bool>(), spot(), 1u8..6, button()).prop_map(
            |(n, corner, to, steps, button)| Op::DragFrom { n, corner, to, steps, button }
        ),
        1 => Just(Op::Leave),
        5 => (key(), proptest::bool::weighted(0.2)).prop_map(|(k, shift)| Op::Key(k, shift)),
        2 => scroll_input_fixed(),
        1 => Just(Op::Blur),
        1 => clipboard(),
        1 => prop_oneof![0u32..20, 20u32..250, 250u32..1500].prop_map(Op::Wait),
    ]
}

/// A sequence of up to `max` ops.
pub fn ops(max: usize) -> impl Strategy<Value = Vec<Op>> {
    proptest::collection::vec(op(), 1..max)
}

/// Every visible node with a layout, in tree order, or only the scroll
/// viewports among them.
fn on_screen(app: &App, viewports_only: bool) -> Vec<WidgetId> {
    let scene = app.scene();
    let mut out = Vec::new();
    let mut stack: Vec<WidgetId> = scene.root().into_iter().collect();
    while let Some(id) = stack.pop() {
        if !scene.is_visible(id) {
            continue;
        }
        let Some(node) = scene.node(id) else { continue };
        stack.extend(node.children.iter().rev().copied());
        let laid = scene
            .layout(id)
            .is_some_and(|l| l.rect.width > 0.0 && l.rect.height > 0.0);
        if !viewports_only && laid || node.kind == WidgetKind::Scroll {
            out.push(id);
        }
    }
    out
}

/// Visible widgets with at least one accessibility action, in tree order.
pub fn actionable(app: &App) -> Vec<WidgetId> {
    let scene = app.scene();
    let mut out = Vec::new();
    let mut stack: Vec<WidgetId> = scene.root().into_iter().collect();
    while let Some(id) = stack.pop() {
        if !scene.is_visible(id) {
            continue;
        }
        if let Some(node) = scene.node(id) {
            stack.extend(node.children.iter().rev().copied());
        }
        let acts = scene.a11y(id).is_some_and(|a| a.actions != 0);
        let laid = scene
            .layout(id)
            .is_some_and(|l| l.rect.width > 0.0 && l.rect.height > 0.0);
        if acts && laid {
            out.push(id);
        }
    }
    out
}

/// Where `Aim { n, corner }` points in `app`: the centre (or a point just
/// inside the top-left corner) of the `n`th actionable widget, wrapping.
/// `None` when nothing is laid out to aim at.
pub fn aim_point(app: &App, n: u8, corner: bool) -> Option<Point> {
    let targets = actionable(app);
    if targets.is_empty() {
        return None;
    }
    let id = targets[n as usize % targets.len()];
    let r = app.scene().layout(id)?.rect;
    Some(if corner {
        Point {
            x: r.x + 2.0,
            y: r.y + 2.0,
        }
    } else {
        Point {
            x: r.x + r.width / 2.0,
            y: r.y + r.height / 2.0,
        }
    })
}

/// `op` with its target resolved against what `app` shows now: an `Aim`
/// becomes a `Wander` and a `Click` a `ClickAt`, so every host that replays
/// the result points at the same place whatever its own layout says. `None`
/// when the op aims and nothing is laid out to aim at. Other ops come back
/// unchanged.
pub fn pinned(app: &App, op: &Op) -> Option<Op> {
    let size = app.size();
    Some(match *op {
        Op::Aim { n, corner } => {
            let p = aim_point(app, n, corner)?;
            Op::Wander(p.x / size.width, p.y / size.height)
        }
        Op::Click {
            n,
            corner,
            button,
            count,
        } => {
            let p = aim_point(app, n, corner)?;
            Op::ClickAt {
                x: p.x / size.width,
                y: p.y / size.height,
                button,
                count,
            }
        }
        Op::TapOn { n, corner } => {
            let p = aim_point(app, n, corner)?;
            Op::Tap(p.x / size.width, p.y / size.height)
        }
        Op::DragFrom {
            n,
            corner,
            to,
            steps,
            button,
        } => {
            let p = aim_point(app, n, corner)?;
            Op::Drag {
                from: (p.x / size.width, p.y / size.height),
                to,
                steps,
                button,
            }
        }
        ref other => other.clone(),
    })
}

/// Moves to `p` and clicks `count` times there, all inside one display frame.
/// A host that was drawing after every event draws once at the end.
pub fn fast_click(host: &mut HeadlessHost, p: Point, button: PointerButton, count: u8) {
    let drawing = !host.batching();
    host.set_batching(true);
    host.pointer_move(p);
    for _ in 0..count {
        host.press(button);
        host.release(button);
    }
    if drawing {
        host.set_batching(false);
    }
}

/// The point of the window that the fractions `fx`, `fy` name.
fn spot(host: &HeadlessHost, fx: f32, fy: f32) -> Point {
    let size = host.app().size();
    Point {
        x: size.width * fx,
        y: size.height * fy,
    }
}

/// Replays one op. `plain` hosts get the undecorated version of a theme.
pub fn apply(host: &mut HeadlessHost, op: &Op, mounted: (u32, u32), plain: bool) {
    match op {
        Op::TapOn { .. } | Op::DragFrom { .. } => {
            if let Some(op) = pinned(host.app(), op) {
                apply(host, &op, mounted, plain);
            }
        }
        Op::Tap(x, y) => {
            let p = spot(host, *x, *y);
            host.click_at(p);
        }
        Op::PressAt(x, y, button) => {
            let p = spot(host, *x, *y);
            host.press_at(p, *button);
        }
        Op::ReleaseAt(x, y, button) => {
            let p = spot(host, *x, *y);
            host.release_at(p, *button);
        }
        Op::Drag {
            from,
            to,
            steps,
            button,
        } => {
            let (from, to) = (spot(host, from.0, from.1), spot(host, to.0, to.1));
            host.drag(from, to, u32::from(*steps), *button);
        }
        Op::Aim { n, corner } => {
            if let Some(p) = aim_point(host.app(), *n, *corner) {
                host.pointer_move(p);
            }
        }
        Op::Wander(fx, fy) => {
            let size = host.app().size();
            host.pointer_move(Point {
                x: size.width * fx,
                y: size.height * fy,
            });
        }
        Op::Press(button) => host.press(*button),
        Op::Release(button) => host.release(*button),
        Op::Click {
            n,
            corner,
            button,
            count,
        } => {
            if let Some(p) = aim_point(host.app(), *n, *corner) {
                fast_click(host, p, *button, *count);
            }
        }
        Op::ClickAt {
            x,
            y,
            button,
            count,
        } => {
            let size = host.app().size();
            fast_click(
                host,
                Point {
                    x: size.width * x,
                    y: size.height * y,
                },
                *button,
                *count,
            );
        }
        Op::Leave => host.pointer_leave(),
        Op::Key(key, shift) => host.key(key, *shift, false),
        Op::Chord(key) => host.key(key, false, true),
        Op::Wheel(dy) => host.wheel(*dy),
        Op::WheelBurst(dy, count) => {
            for _ in 0..*count {
                host.wheel(*dy);
            }
        }
        Op::WheelAt(fx, fy, dy) => {
            let size = host.app().size();
            host.wheel_at(
                Point {
                    x: size.width * fx,
                    y: size.height * fy,
                },
                *dy,
            );
        }
        Op::AimBar { n, part } => {
            let bars = scrollbars(host.app());
            if bars.is_empty() {
                return;
            }
            let bar = bars[*n as usize % bars.len()];
            let (track, thumb) = (bar.track, bar.thumb);
            let x = track.x + track.width / 2.0;
            let y = match part {
                BarPart::Thumb => thumb.y + thumb.height / 2.0,
                BarPart::Above if thumb.y > track.y + 2.0 => (track.y + thumb.y) / 2.0,
                BarPart::Below if thumb.bottom() < track.bottom() - 2.0 => {
                    (thumb.bottom() + track.bottom()) / 2.0
                }
                _ => thumb.y + thumb.height / 2.0,
            };
            host.pointer_move(Point { x, y });
        }
        Op::DragBar { n, to, drift } => {
            let bars = scrollbars(host.app());
            if bars.is_empty() || host.held() {
                return;
            }
            let bar = bars[*n as usize % bars.len()];
            let (track, thumb) = (bar.track, bar.thumb);
            let x = track.x + track.width / 2.0;
            let start = Point {
                x,
                y: thumb.y + thumb.height / 2.0,
            };
            // The pointer lands where the thumb's centre should end up.
            let travel = (track.height - thumb.height).max(0.0);
            let end = Point {
                x: x + drift,
                y: track.y + thumb.height / 2.0 + travel * to,
            };
            host.pointer_move(start);
            host.press(PointerButton::Left);
            for step in 1..=3 {
                let t = step as f32 / 3.0;
                host.pointer_move(Point {
                    x: start.x + (end.x - start.x) * t,
                    y: start.y + (end.y - start.y) * t,
                });
            }
            host.release(PointerButton::Left);
        }
        Op::Access {
            n,
            viewport,
            action,
        } => {
            let targets = on_screen(host.app(), *viewport);
            if targets.is_empty() {
                return;
            }
            let id = targets[*n as usize % targets.len()];
            host.dispatch_action(&ActionRequest {
                action: action.action(),
                target_tree: TreeId::ROOT,
                target_node: schnellui::a11y::to_access_id(id),
                data: None,
            });
        }
        Op::Resize(fw, fh) => host.resize(
            (mounted.0 as f32 * fw).round().max(crate::curve::MIN.0),
            (mounted.1 as f32 * fh).round().max(crate::curve::MIN.1),
        ),
        Op::Blur => host.blur(),
        Op::Clipboard(action) => host.clipboard(*action),
        Op::Theme(i) => {
            let theme = THEMES[i % THEMES.len()];
            host.set_theme(if plain {
                schnellui::testing::fuzz::strip_interactions(theme)
            } else {
                theme
            });
        }
        Op::Wait(ms) => host.advance(Duration::from_millis(u64::from(*ms))),
        Op::Burst(on) => host.set_batching(*on),
        Op::Frame => host.frame(),
        Op::ThemeFade(i, ms) => {
            let theme = THEMES[i % THEMES.len()];
            let theme = if plain {
                schnellui::testing::fuzz::strip_interactions(theme)
            } else {
                theme
            };
            host.transition_theme(theme, Duration::from_millis(u64::from(*ms)));
        }
    }
}

/// Takes the pointer away, lets go of both buttons, closes menus and
/// clears focus, so no hover, press or focus decoration should remain.
pub fn quiesce(host: &mut HeadlessHost) {
    host.set_batching(false);
    host.release(PointerButton::Left);
    host.release(PointerButton::Right);
    host.pointer_leave();
    host.key(&HostKey::Escape, false, false);
    host.app_mut().dismiss_context_menu();
    host.blur();
    finish_animations(host);
}

/// Advances virtual time a display frame at a time until every animation
/// has ended (at most ten seconds).
pub fn finish_animations(host: &mut HeadlessHost) {
    host.run_animations(Duration::from_millis(16), Duration::from_secs(10), |_| {});
}
