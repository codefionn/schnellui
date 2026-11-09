//! Input a person can produce, as proptest strategies, and how a host
//! replays it.

use proptest::prelude::*;
use schnellui::scene::{Point, WidgetId};
use schnellui::App;

use crate::{HeadlessHost, HostKey, PointerButton, THEMES};

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
    Leave,
    Key(HostKey, bool),
    Wheel(f32),
    /// Resize to fractions of the mounted size.
    Resize(f32, f32),
    Blur,
    Theme(usize),
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

/// Any single op, weighted towards pointing and pressing.
pub fn op() -> impl Strategy<Value = Op> {
    let button = prop_oneof![4 => Just(PointerButton::Left), 1 => Just(PointerButton::Right)];
    prop_oneof![
        8 => (any::<u8>(), any::<bool>()).prop_map(|(n, corner)| Op::Aim { n, corner }),
        2 => (0.0f32..1.0, 0.0f32..1.0).prop_map(|(x, y)| Op::Wander(x, y)),
        4 => button.clone().prop_map(Op::Press),
        4 => button.prop_map(Op::Release),
        1 => Just(Op::Leave),
        6 => (key(), proptest::bool::weighted(0.2)).prop_map(|(k, shift)| Op::Key(k, shift)),
        1 => prop_oneof![Just(-120.0f32), Just(-40.0), Just(40.0), Just(120.0)].prop_map(Op::Wheel),
        1 => (0.2f32..1.3, 0.2f32..1.3).prop_map(|(w, h)| Op::Resize(w, h)),
        1 => Just(Op::Blur),
        1 => (0..THEMES.len()).prop_map(Op::Theme),
    ]
}

/// A sequence of up to `max` ops.
pub fn ops(max: usize) -> impl Strategy<Value = Vec<Op>> {
    proptest::collection::vec(op(), 1..max)
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

/// Replays one op. `plain` hosts get the undecorated version of a theme.
pub fn apply(host: &mut HeadlessHost, op: &Op, mounted: (u32, u32), plain: bool) {
    match op {
        Op::Aim { n, corner } => {
            let targets = actionable(host.app());
            if targets.is_empty() {
                return;
            }
            let id = targets[*n as usize % targets.len()];
            let Some(r) = host.app().scene().layout(id).map(|l| l.rect) else {
                return;
            };
            let p = if *corner {
                Point {
                    x: r.x + 2.0,
                    y: r.y + 2.0,
                }
            } else {
                Point {
                    x: r.x + r.width / 2.0,
                    y: r.y + r.height / 2.0,
                }
            };
            host.pointer_move(p);
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
        Op::Leave => host.pointer_leave(),
        Op::Key(key, shift) => host.key(key, *shift, false),
        Op::Wheel(dy) => host.wheel(*dy),
        Op::Resize(fw, fh) => host.resize(
            (mounted.0 as f32 * fw).round().max(crate::curve::MIN.0),
            (mounted.1 as f32 * fh).round().max(crate::curve::MIN.1),
        ),
        Op::Blur => host.blur(),
        Op::Theme(i) => {
            let theme = THEMES[i % THEMES.len()];
            host.set_theme(if plain {
                schnellui::testing::fuzz::strip_interactions(theme)
            } else {
                theme
            });
        }
    }
}

/// Takes the pointer away, lets go of both buttons, closes menus and
/// clears focus, so no hover, press or focus decoration should remain.
pub fn quiesce(host: &mut HeadlessHost) {
    host.release(PointerButton::Left);
    host.release(PointerButton::Right);
    host.pointer_leave();
    host.key(&HostKey::Escape, false, false);
    host.app_mut().dismiss_context_menu();
    host.blur();
}
