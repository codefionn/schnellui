//! Additional basic content leaves (SOUL §8.1): [`ProgressBar`],
//! [`LoadingSpinner`], [`Switch`], [`Radio`], [`Divider`], [`Link`], [`Badge`].
//! Same contract as the leaves in the crate root: they draw pixels, **always**
//! carry an AccessKit role +
//! name/value/state/actions (SOUL §6.1), and route content input through the *same*
//! inbound path as an AccessKit `ActionRequest` (SOUL §6.3). Each builds a
//! [`WidgetKind`] content leaf (`is_container() == false`), emits its paint via
//! [`crate::node_rect`], registers an intrinsic-size measure, and stores its
//! handlers in the crate-root widget-interaction registry.

use std::borrow::Cow;

use schnellui_a11y::{ActionFlags, Role, StateFlags};
use schnellui_motion::{Easing, Motion, Property as MotionProperty, Repeat as MotionRepeat};
use schnellui_scene::{DirtyFlags, Point, Primitive, Rect, Scene, Size, WidgetId, WidgetKind};
use smallvec::SmallVec;

mod controls;
use crate::{
    node_rect, norm_scale, phys_size_px, rasterize_and_push, theme_for, with_handlers,
    write_text_semantics, BuildCtx, ClickHandler, SpinnerState, View,
};
pub use controls::*;

// ---------------------------------------------------------------------------
// visual + metric constants (deterministic for shots, SOUL §7.3)
// ---------------------------------------------------------------------------

/// Progress-bar intrinsic width / height (SOUL §8.1).
const PROGRESS_WIDTH: f32 = 160.0;
const PROGRESS_HEIGHT: f32 = 8.0;
/// The rounded-rect radius shared by the progress track + fill.
const PROGRESS_RADIUS: f32 = 4.0;
/// Default loading-spinner edge and number of radial strokes.
const SPINNER_SIZE: f32 = 24.0;
const SPINNER_SEGMENTS: usize = 12;
/// The spinner rotation, declared once for every backend: the GPU path samples
/// it against a monotonic clock (frame-rate independent), the HTML renderer
/// compiles it to the CSS `@keyframes`/`animation` pair. One revolution per
/// 900ms, linear, forever.
pub(crate) const SPINNER_MOTION: Motion = Motion {
    property: MotionProperty::Rotate { turns: 1.0 },
    duration_ms: 900.0,
    easing: Easing::Linear,
    repeat: MotionRepeat::Infinite,
    delay_ms: 0.0,
};
/// Nominal per-frame advance used when no clock is available (headless tests).
pub(crate) const SPINNER_FRAME_MS: f32 = 75.0;

/// Switch classic track size (SOUL §8.1); knob + radii derive from the laid-out
/// rect under the ambient [`Shape`](crate::Shape) tokens.
const SWITCH_WIDTH: f32 = 36.0;
const SWITCH_HEIGHT: f32 = 20.0;
/// The knob's inset from the track edge (also fixes the knob's size).
const SWITCH_INSET: f32 = 2.0;
/// The knob's classic edge (track height − 2 × inset) — tests derive expected
/// slide offsets from it.
#[cfg(test)]
const SWITCH_KNOB: f32 = SWITCH_HEIGHT - 2.0 * SWITCH_INSET;

/// Radio classic circle size (SOUL §8.1); ring/inner/dot radii derive from the
/// laid-out rect under the ambient [`Shape`](crate::Shape) tokens.
const RADIO_SIZE: f32 = 18.0;
const RADIO_INSET: f32 = 2.0;
const RADIO_DOT: f32 = 8.0;

/// Link text colour (SOUL §8.1) — the framework blue, shared with buttons so the
/// Link label font size.
const LINK_TEXT_SIZE: f32 = 16.0;
/// Gap between the text run's bottom edge and the underline hairline.
const LINK_UNDERLINE_GAP: f32 = 1.0;
/// Underline hairline thickness.
const LINK_UNDERLINE: f32 = 1.0;

/// Badge label font size.
const BADGE_TEXT_SIZE: f32 = 12.0;
/// Badge horizontal padding (each side).
const BADGE_PAD_H: f32 = 6.0;
/// Badge vertical padding (each side).
const BADGE_PAD_V: f32 = 2.0;

// ---------------------------------------------------------------------------
// paint-fragment emission (SOUL §3.2, §8.1 — cleared-and-refilled, §4.4)
// ---------------------------------------------------------------------------

/// The clamped `[0,1]` fill fraction of a range (SOUL §8.1). Zero for a degenerate
/// (`max <= min`) range so a bad input never divides by zero or overflows.
#[inline]
fn clamp_fraction(value: f32, min: f32, max: f32) -> f32 {
    if max > min {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Emits a progress bar's paint: a full-width track plus a blue fill proportional to
/// `frac` (SOUL §8.1). Cleared-and-refilled in place (§4.4).
fn emit_progress_paint(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    id: WidgetId,
    frac: f32,
    intrinsic: Size,
) {
    let rect = node_rect(scene, id, intrinsic);
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    let t = theme_for(runtime, id);
    let radius = t.shape.radius(PROGRESS_RADIUS, rect.height);
    pd.primitives.push(Primitive::SolidRect {
        rect,
        color: t.media,
        corner_radius: radius,
    });
    pd.primitives.push(Primitive::SolidRect {
        rect: Rect::new(rect.x, rect.y, rect.width * frac, rect.height),
        color: t.accent,
        corner_radius: radius,
    });
}

/// Emits one deterministic frame of the indeterminate loading spinner. A transparent
/// bounds quad keeps the radial strokes anchored to the exact layout box while the
/// highlighted segment rotates. The fragment is cleared-and-refilled in place, so a
/// steady animation frame reuses its original allocation (SOUL §4.4).
pub(crate) fn emit_spinner_paint(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    id: WidgetId,
    size: f32,
    phase: f32,
) {
    let edge = size.max(8.0);
    let intrinsic = Size {
        width: edge,
        height: edge,
    };
    let rect = node_rect(scene, id, intrinsic);
    let center = Point {
        x: rect.x + rect.width * 0.5,
        y: rect.y + rect.height * 0.5,
    };
    let outer = rect.width.min(rect.height) * 0.46;
    let inner = outer * 0.58;
    let stroke = (edge * 0.095).max(1.5);
    let accent = theme_for(runtime, id).accent;
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    pd.primitives.push(Primitive::SolidRect {
        rect,
        color: schnellui_scene::Color::TRANSPARENT,
        corner_radius: 0.0,
    });
    // `phase` is the continuous [0,1) loop progress of SPINNER_MOTION. The
    // highlight rotates smoothly with it; each segment's fade distance is
    // measured in fractional segment units so intermediate phases produce
    // intermediate alpha ramps rather than snapping between discrete frames.
    let phase = phase.clamp(0.0, 1.0);
    let highlight = phase * SPINNER_SEGMENTS as f32;
    for segment in 0..SPINNER_SEGMENTS {
        let angle = std::f32::consts::TAU * segment as f32 / SPINNER_SEGMENTS as f32
            - std::f32::consts::FRAC_PI_2;
        let distance = (segment as f32 - highlight)
            .abs()
            .min(SPINNER_SEGMENTS as f32 - (segment as f32 - highlight).abs());
        let alpha = (255.0 - distance * 17.0).max(55.0) as u8;
        let color = schnellui_scene::Color {
            a: ((accent.a as u16 * alpha as u16) / 255) as u8,
            ..accent
        };
        let (sin, cos) = angle.sin_cos();
        pd.primitives.push(Primitive::Line {
            from: Point {
                x: center.x + cos * inner,
                y: center.y + sin * inner,
            },
            to: Point {
                x: center.x + cos * outer,
                y: center.y + sin * outer,
            },
            width: stroke,
            color,
        });
    }
}

/// A switch's intrinsic box: the classic track scaled by the density token
/// (SOUL §8.1) — a chunky design system gets a physically bigger switch.
fn switch_intrinsic(runtime: &crate::Runtime, id: WidgetId) -> Size {
    let sh = theme_for(runtime, id).shape;
    Size {
        width: sh.pad(SWITCH_WIDTH),
        height: sh.pad(SWITCH_HEIGHT),
    }
}

/// Emits a switch's paint: a pill track (blue when `on`, grey when off) plus a white
/// knob, slid to the right when `on` and the left when off (SOUL §8.1). Geometry
/// derives from the laid-out rect — the density-scaled box keeps its proportions —
/// and the pill radii square out under a low-roundness design ([`Shape::pill`]).
fn emit_switch_paint(runtime: &crate::Runtime, scene: &mut Scene, id: WidgetId, on: bool) {
    let rect = node_rect(scene, id, switch_intrinsic(runtime, id));
    let t = theme_for(runtime, id);
    let track_color = if on { t.accent } else { t.media };
    let knob = (rect.height - 2.0 * SWITCH_INSET).max(0.0);
    let knob_x = if on {
        rect.x + rect.width - SWITCH_INSET - knob
    } else {
        rect.x + SWITCH_INSET
    };
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    pd.primitives.push(Primitive::SolidRect {
        rect,
        color: track_color,
        corner_radius: t.shape.pill(rect.height),
    });
    pd.primitives.push(Primitive::SolidRect {
        rect: Rect::new(knob_x, rect.y + SWITCH_INSET, knob, knob),
        color: t.surface,
        corner_radius: t.shape.pill(knob),
    });
}

/// A radio's intrinsic box: the classic circle scaled by the density token.
fn radio_intrinsic(runtime: &crate::Runtime, id: WidgetId) -> Size {
    let edge = theme_for(runtime, id).shape.pad(RADIO_SIZE);
    Size {
        width: edge,
        height: edge,
    }
}

/// Emits a radio's paint: a grey outer ring, a white inner circle inset 2px, and —
/// when `selected` — a centred blue dot (SOUL §8.1). All circles are [`Shape::pill`]
/// radii, so a squared design system turns the radio into concentric blocks while
/// the inner dot still reads as the selection mark.
fn emit_radio_paint(runtime: &crate::Runtime, scene: &mut Scene, id: WidgetId, selected: bool) {
    let rect = node_rect(scene, id, radio_intrinsic(runtime, id));
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    let t = theme_for(runtime, id);
    let inner = (rect.width - 2.0 * RADIO_INSET).max(0.0);
    // outer ring
    pd.primitives.push(Primitive::SolidRect {
        rect,
        color: t.media,
        corner_radius: t.shape.pill(rect.height),
    });
    // inner white circle, inset by 2px on each side
    pd.primitives.push(Primitive::SolidRect {
        rect: Rect::new(rect.x + RADIO_INSET, rect.y + RADIO_INSET, inner, inner),
        color: t.surface,
        corner_radius: t.shape.pill(inner),
    });
    if selected {
        // centred dot, proportional to the (possibly density-scaled) box
        let dot = rect.width * (RADIO_DOT / RADIO_SIZE);
        let dot_off = (rect.width - dot) * 0.5;
        pd.primitives.push(Primitive::SolidRect {
            rect: Rect::new(rect.x + dot_off, rect.y + dot_off, dot, dot),
            color: t.accent,
            corner_radius: t.shape.pill(dot),
        });
    }
}

/// Emits a divider's hairline (SOUL §8.1). The rect spans the node's laid-out width
/// via [`node_rect`]; before the first layout pass it is provisional (§8.1 pass
/// order — paint reads geometry only *after* layout runs).
fn emit_divider_paint(runtime: &crate::Runtime, scene: &mut Scene, id: WidgetId, thickness: f32) {
    let intrinsic = Size {
        width: 0.0,
        height: thickness,
    };
    let rect = node_rect(scene, id, intrinsic);
    let pd = scene.paint_mut(id);
    pd.primitives.clear();
    pd.primitives.push(Primitive::SolidRect {
        rect,
        color: theme_for(runtime, id).separator,
        corner_radius: 0.0,
    });
}

// ---------------------------------------------------------------------------
// state helpers (SOUL §6.1 — packed CHECKED bit in the a11y column)
// ---------------------------------------------------------------------------

/// Flips `id`'s CHECKED bit and returns the new value (SOUL §6.1). Writes the a11y
/// column directly — the caller marks A11Y dirty (mirrors the checkbox arm's
/// discipline in the crate root).
fn toggle_checked(scene: &mut Scene, id: WidgetId) -> bool {
    let a = scene.a11y_mut(id);
    let mut s = StateFlags(a.state);
    let now = !s.contains(StateFlags::CHECKED);
    if now {
        s.insert(StateFlags::CHECKED);
    } else {
        s.0 &= !StateFlags::CHECKED.0;
    }
    a.state = s.0;
    now
}

/// Sets `id`'s CHECKED bit (SOUL §6.1).
fn set_checked(scene: &mut Scene, id: WidgetId) {
    let a = scene.a11y_mut(id);
    let mut s = StateFlags(a.state);
    s.insert(StateFlags::CHECKED);
    a.state = s.0;
}

/// Clears `id`'s CHECKED bit (SOUL §6.1).
fn clear_checked(scene: &mut Scene, id: WidgetId) {
    let a = scene.a11y_mut(id);
    let mut s = StateFlags(a.state);
    s.0 &= !StateFlags::CHECKED.0;
    a.state = s.0;
}

/// `true` if `id`'s a11y column carries the CHECKED bit (SOUL §6.1).
#[inline]
fn is_checked(scene: &Scene, id: WidgetId) -> bool {
    scene
        .a11y(id)
        .map(|a| StateFlags(a.state).contains(StateFlags::CHECKED))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// ProgressBar (SOUL §8.1 — a read-only range status)
// ---------------------------------------------------------------------------

/// A determinate progress bar (SOUL §8.1). `Role::ProgressIndicator`; its accessible
/// value is the percentage the fill represents. Read-only — it advertises no actions.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{reset, Row};
    use schnellui_layout::LayoutEngine;
    use schnellui_scene::Color;
    use schnellui_signal::create_signal;
    use schnellui_text::{GlyphAtlas, TextShaper};

    /// Builds `view` into a fresh scene as the root, returning the scene, layout
    /// engine, pooled shaper + glyph atlas, and the root id (mirrors the private
    /// `build_one` in the crate-root tests).
    fn build_one(
        runtime: &crate::Runtime,
        view: impl View,
    ) -> (Scene, LayoutEngine, TextShaper, GlyphAtlas, WidgetId) {
        reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(512, 512);
        let id = {
            let mut ctx = BuildCtx {
                context: crate::Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            Box::new(view).build(&mut ctx, None)
        };
        scene.set_root(id);
        (scene, layout, text, atlas, id)
    }

    /// The `SolidRect` rect of the primitive at `idx` on a node (panics otherwise).
    fn rect_of(scene: &Scene, id: WidgetId, idx: usize) -> Rect {
        match scene.paint(id).unwrap().primitives[idx] {
            Primitive::SolidRect { rect, .. } => rect,
            ref p => panic!("expected a SolidRect, got {p:?}"),
        }
    }

    // --- build-time semantics (SOUL §6.1 — no widget without a role) ---

    #[test]
    fn every_basic_widget_reports_its_role_and_kind() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        assert_eq!(
            ProgressBar::new(0.0, 0.0, 1.0).role(),
            Role::ProgressIndicator
        );
        assert_eq!(
            ProgressBar::new(0.0, 0.0, 1.0).kind(),
            WidgetKind::ProgressBar
        );
        assert_eq!(LoadingSpinner::new().role(), Role::ProgressIndicator);
        assert_eq!(LoadingSpinner::new().kind(), WidgetKind::LoadingSpinner);
        assert_eq!(Switch::new(false).role(), Role::Switch);
        assert_eq!(Switch::new(false).kind(), WidgetKind::Switch);
        assert_eq!(Radio::new(false).role(), Role::Radio);
        assert_eq!(Radio::new(false).kind(), WidgetKind::Radio);
        assert_eq!(Divider::new().role(), Role::Group);
        assert_eq!(Divider::new().kind(), WidgetKind::Divider);
    }

    #[test]
    fn progressbar_build_carries_role_value_and_no_actions() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(runtime, ProgressBar::new(50.0, 0.0, 100.0));
        let a = scene.a11y(id).expect("a11y column written at build");
        assert_eq!(Role::from_u16(a.role), Role::ProgressIndicator);
        assert_eq!(a.value.as_deref(), Some("50%"));
        assert_eq!(a.actions, 0, "a progress bar advertises no actions");
        // paint: a full track + a proportional fill (SOUL §8.1).
        assert_eq!(scene.paint(id).unwrap().primitives.len(), 2);
    }

    #[test]
    fn progressbar_clamps_and_formats_exact_percent_strings() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        for (value, min, max, want) in [
            (0.0f32, 0.0f32, 100.0f32, "0%"),
            (50.0, 0.0, 100.0, "50%"),
            (100.0, 0.0, 100.0, "100%"),
            (-25.0, 0.0, 100.0, "0%"),   // clamp below min
            (250.0, 0.0, 100.0, "100%"), // clamp above max
            (0.5, 0.0, 1.0, "50%"),      // fractional range
            (5.0, 5.0, 5.0, "0%"),       // degenerate range → 0
        ] {
            let (scene, _l, _t, _a, id) = build_one(runtime, ProgressBar::new(value, min, max));
            assert_eq!(
                scene.a11y(id).unwrap().value.as_deref(),
                Some(want),
                "value={value} min={min} max={max}"
            );
        }
    }

    #[test]
    fn progressbar_fill_width_is_proportional() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(runtime, ProgressBar::new(25.0, 0.0, 100.0));
        let track = rect_of(&scene, id, 0);
        let fill = rect_of(&scene, id, 1);
        assert_eq!(track.width, PROGRESS_WIDTH);
        assert!((fill.width - PROGRESS_WIDTH * 0.25).abs() < 0.001);
    }

    #[test]
    fn progressbar_supports_accessible_names_and_custom_size() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(
            runtime,
            ProgressBar::new(3.0, 0.0, 4.0)
                .name("Downloading")
                .size(240.0, 12.0),
        );
        let a = scene.a11y(id).unwrap();
        assert_eq!(a.name.as_deref(), Some("Downloading"));
        assert_eq!(a.value.as_deref(), Some("75%"));
        let track = rect_of(&scene, id, 0);
        assert_eq!((track.width, track.height), (240.0, 12.0));
    }

    #[test]
    fn loading_spinner_is_semantic_and_draws_a_faded_radial_frame() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(
            runtime,
            LoadingSpinner::new().name("Synchronizing").size(32.0),
        );
        let a = scene.a11y(id).unwrap();
        assert_eq!(Role::from_u16(a.role), Role::ProgressIndicator);
        assert_eq!(a.name.as_deref(), Some("Synchronizing"));
        assert_eq!(a.actions, 0);
        let primitives = &scene.paint(id).unwrap().primitives;
        assert_eq!(primitives.len(), SPINNER_SEGMENTS + 1);
        assert!(matches!(
            primitives[0],
            Primitive::SolidRect {
                color: schnellui_scene::Color::TRANSPARENT,
                ..
            }
        ));
        let alphas: Vec<u8> = primitives[1..]
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::Line { color, .. } => Some(color.a),
                _ => None,
            })
            .collect();
        assert_eq!(alphas.len(), SPINNER_SEGMENTS);
        assert!(alphas.iter().max() > alphas.iter().min());
    }

    #[test]
    fn switch_build_carries_role_state_and_actions() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(runtime, Switch::new(true));
        let a = scene.a11y(id).unwrap();
        assert_eq!(Role::from_u16(a.role), Role::Switch);
        assert!(StateFlags(a.state).contains(StateFlags::CHECKED));
        assert!(ActionFlags(a.actions).contains(ActionFlags::CLICK));
        assert!(ActionFlags(a.actions).contains(ActionFlags::FOCUS));
        // an off switch carries no CHECKED bit
        let (scene, _l, _t, _a, id) = build_one(runtime, Switch::new(false));
        assert!(!StateFlags(scene.a11y(id).unwrap().state).contains(StateFlags::CHECKED));
    }

    #[test]
    fn switch_knob_slides_with_state() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene_off, _l, _t, _a, off) = build_one(runtime, Switch::new(false));
        let (scene_on, _l2, _t2, _a2, on) = build_one(runtime, Switch::new(true));
        let knob_off = rect_of(&scene_off, off, 1);
        let knob_on = rect_of(&scene_on, on, 1);
        assert_eq!(knob_off.x, SWITCH_INSET, "off knob sits at the left inset");
        assert_eq!(
            knob_on.x,
            SWITCH_WIDTH - SWITCH_INSET - SWITCH_KNOB,
            "on knob sits at the right inset"
        );
        assert!(knob_on.x > knob_off.x, "the knob moves right when on");
    }

    // --- input handling: pointer and ActionRequest converge (SOUL §6.3) ---

    #[test]
    fn switch_click_toggles_state_paint_and_fires_handler() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let last = create_signal(false);
        let (mut scene, _l, _t, _a, id) =
            build_one(runtime, Switch::new(false).on_toggle(move |b| last.set(b)));
        assert!(!StateFlags(scene.a11y(id).unwrap().state).contains(StateFlags::CHECKED));

        // the same inbound path a `Click` ActionRequest takes (SOUL §6.3)
        assert!(crate::dispatch_click(runtime, &mut scene, id));
        assert!(StateFlags(scene.a11y(id).unwrap().state).contains(StateFlags::CHECKED));
        assert!(last.get());
        assert!(scene.dirty_flags(id).contains(DirtyFlags::PAINT));
        assert!(scene.dirty_flags(id).contains(DirtyFlags::A11Y));
        // paint re-emitted for the on-state: knob slid to the right inset
        assert_eq!(
            rect_of(&scene, id, 1).x,
            SWITCH_WIDTH - SWITCH_INSET - SWITCH_KNOB
        );

        // toggle back off
        assert!(crate::dispatch_click(runtime, &mut scene, id));
        assert!(!StateFlags(scene.a11y(id).unwrap().state).contains(StateFlags::CHECKED));
        assert!(!last.get());
        assert_eq!(rect_of(&scene, id, 1).x, SWITCH_INSET);
    }

    #[test]
    fn radio_build_carries_role_state_and_actions() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(runtime, Radio::new(true));
        let a = scene.a11y(id).unwrap();
        assert_eq!(Role::from_u16(a.role), Role::Radio);
        assert!(StateFlags(a.state).contains(StateFlags::CHECKED));
        assert!(ActionFlags(a.actions).contains(ActionFlags::CLICK));
        assert!(ActionFlags(a.actions).contains(ActionFlags::FOCUS));
        // a selected radio paints ring + inner circle + dot; unselected omits the dot
        assert_eq!(scene.paint(id).unwrap().primitives.len(), 3);
        let (scene, _l, _t, _a, id) = build_one(runtime, Radio::new(false));
        assert_eq!(scene.paint(id).unwrap().primitives.len(), 2);
    }

    #[test]
    fn radio_group_click_is_exclusive_and_marks_both_dirty() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        // Two radios under one Row parent (SOUL §6.3 radio-group exclusivity).
        reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(512, 512);
        let root = {
            let mut ctx = BuildCtx {
                context: crate::Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            Box::new(Row::new().child(Radio::new(true)).child(Radio::new(false)))
                .build(&mut ctx, None)
        };
        scene.set_root(root);
        let kids = scene.node(root).unwrap().children.clone();
        let (r0, r1) = (kids[0], kids[1]);
        assert!(is_checked(&scene, r0));
        assert!(!is_checked(&scene, r1));

        scene.clear_dirty();
        // click the second radio → exclusivity flips the group
        assert!(crate::dispatch_click(runtime, &mut scene, r1));
        assert!(!is_checked(&scene, r0), "the first radio was cleared");
        assert!(is_checked(&scene, r1), "the clicked radio is now selected");
        // both the cleared and the newly-selected radio are marked dirty
        for r in [r0, r1] {
            assert!(
                scene.dirty_flags(r).contains(DirtyFlags::A11Y),
                "{r:?} a11y"
            );
            assert!(
                scene.dirty_flags(r).contains(DirtyFlags::PAINT),
                "{r:?} paint"
            );
        }
        // the cleared radio dropped its dot (2 prims), the selected gained one (3)
        assert_eq!(scene.paint(r0).unwrap().primitives.len(), 2);
        assert_eq!(scene.paint(r1).unwrap().primitives.len(), 3);
    }

    #[test]
    fn radio_select_fires_handler_and_already_selected_refires() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let count = create_signal(0i32);
        let (mut scene, _l, _t, _a, id) = build_one(
            runtime,
            Radio::new(false).on_select(move || count.update(|v| *v += 1)),
        );
        assert!(!is_checked(&scene, id));
        // first click selects + fires
        assert!(crate::dispatch_click(runtime, &mut scene, id));
        assert!(is_checked(&scene, id));
        assert_eq!(count.get(), 1);
        // an already-selected radio still fires its handler without re-changing state
        assert!(crate::dispatch_click(runtime, &mut scene, id));
        assert_eq!(count.get(), 2);
        assert!(is_checked(&scene, id));
    }

    // --- divider: decorative + width-spanning measure (SOUL §8.1) ---

    #[test]
    fn divider_is_decorative_and_measures_available_width_by_thickness() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(64, 64);
        let id = {
            let mut ctx = BuildCtx {
                context: crate::Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            Box::new(Divider::new().thickness(2.0)).build(&mut ctx, None)
        };
        scene.set_root(id);

        // decorative Group: no name, value, or actions (SOUL §6.1)
        let a = scene.a11y(id).unwrap();
        assert_eq!(Role::from_u16(a.role), Role::Group);
        assert!(a.name.is_none());
        assert!(a.value.is_none());
        assert_eq!(a.actions, 0);

        // the measure spans the offered width at the requested thickness (SOUL §8.1)
        layout.sync_tree(&scene, id);
        layout.compute(
            &mut scene,
            id,
            Size {
                width: 200.0,
                height: 100.0,
            },
        );
        let rect = scene.layout(id).unwrap().rect;
        assert_eq!(rect.width, 200.0, "divider spans the available width");
        assert_eq!(rect.height, 2.0, "divider height is its thickness");
    }

    #[test]
    fn divider_thickness_defaults_to_one() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, mut layout, _t, _a, id) = build_one(runtime, Divider::new());
        layout.sync_tree(&scene, id);
        layout.compute(
            &mut scene,
            id,
            Size {
                width: 120.0,
                height: 40.0,
            },
        );
        assert_eq!(scene.layout(id).unwrap().rect.height, 1.0);
    }

    /// After layout, `reposition_paint` anchors the divider's hairline to its full
    /// laid-out rect (the width-spanning special case in `reposition_node`, §8.1) —
    /// the build-time paint is provisional zero-width and would otherwise stay
    /// invisible.
    #[test]
    fn divider_paint_spans_laid_out_width_after_reposition() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, mut layout, _t, _a, id) = build_one(runtime, Divider::new().thickness(2.0));
        layout.sync_tree(&scene, id);
        layout.compute(
            &mut scene,
            id,
            Size {
                width: 200.0,
                height: 40.0,
            },
        );
        crate::reposition_paint(runtime, &mut scene);
        let prims = &scene.paint(id).unwrap().primitives;
        let Primitive::SolidRect { rect, .. } = prims[0] else {
            panic!("divider paints a SolidRect hairline");
        };
        assert_eq!(rect.width, 200.0, "hairline adopts the laid-out width");
        assert_eq!(rect.height, 2.0, "hairline keeps its emitted thickness");
        // idempotent: a second pass writes nothing new
        crate::reposition_paint(runtime, &mut scene);
        let Primitive::SolidRect { rect: again, .. } = scene.paint(id).unwrap().primitives[0]
        else {
            panic!("still a SolidRect");
        };
        assert_eq!(again, rect);
    }

    // --- link: role + underline paint + button-like activation (SOUL §6.3) ---

    #[test]
    fn link_build_carries_role_name_actions_and_underlined_glyphs() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(runtime, Link::new("docs"));
        let a = scene.a11y(id).expect("a11y column written at build");
        assert_eq!(Role::from_u16(a.role), Role::Link);
        assert_eq!(a.name.as_deref(), Some("docs"));
        assert!(ActionFlags(a.actions).contains(ActionFlags::CLICK));
        assert!(ActionFlags(a.actions).contains(ActionFlags::FOCUS));
        // paint: an underline hairline + real glyph quads, all in link blue
        let prims = &scene.paint(id).unwrap().primitives;
        let Primitive::SolidRect { rect, color, .. } = prims[0] else {
            panic!("link underline is a SolidRect hairline");
        };
        assert_eq!(color, crate::Theme::default().accent);
        assert_eq!(rect.height, LINK_UNDERLINE);
        assert!(rect.width > 0.0, "underline spans the shaped run");
        assert!(
            prims[1..]
                .iter()
                .all(|p| matches!(p, Primitive::GlyphQuad { color, .. } if *color == crate::Theme::default().accent)),
            "link label renders as blue glyph quads"
        );
    }

    #[test]
    fn link_click_fires_handler_and_disabled_link_is_inert() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let count = create_signal(0i32);
        let (mut scene, _l, _t, _a, id) = build_one(
            runtime,
            Link::new("docs").on_click(move || count.update(|v| *v += 1)),
        );
        // the same inbound path a `Click` ActionRequest takes (SOUL §6.3)
        assert!(crate::dispatch_click(runtime, &mut scene, id));
        assert_eq!(count.get(), 1);

        let count2 = create_signal(0i32);
        let (mut scene, _l, _t, _a, id) = build_one(
            runtime,
            Link::new("docs")
                .disabled(true)
                .on_click(move || count2.update(|v| *v += 1)),
        );
        assert!(StateFlags(scene.a11y(id).unwrap().state).contains(StateFlags::DISABLED));
        assert!(!crate::dispatch_click(runtime, &mut scene, id));
        assert_eq!(count2.get(), 0);
    }

    #[test]
    fn link_measures_text_plus_underline() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, mut layout, _t, _a, id) = build_one(runtime, Link::new("docs"));
        layout.sync_tree(&scene, id);
        layout.compute(
            &mut scene,
            id,
            Size {
                width: 400.0,
                height: 100.0,
            },
        );
        let rect = scene.layout(id).unwrap().rect;
        assert!(rect.width > 0.0);
        assert!(rect.height > LINK_UNDERLINE_GAP + LINK_UNDERLINE);
    }

    // --- badge: a status pill announcing its text as a live value (SOUL §6.2) ---

    #[test]
    fn badge_build_carries_status_value_pill_and_no_actions() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) = build_one(runtime, Badge::new("3"));
        let a = scene.a11y(id).expect("a11y column written at build");
        assert_eq!(Role::from_u16(a.role), Role::Status);
        assert_eq!(a.value.as_deref(), Some("3"));
        assert_eq!(a.actions, 0, "a badge advertises no actions");
        // paint: pill background + white glyph quads
        let prims = &scene.paint(id).unwrap().primitives;
        let Primitive::SolidRect {
            color,
            corner_radius,
            rect,
        } = prims[0]
        else {
            panic!("badge pill is a SolidRect");
        };
        assert_eq!(color, crate::Theme::default().attention);
        // full-height radius rounds the pill ends
        assert!((corner_radius - rect.height * 0.5).abs() < 0.001);
        assert!(prims[1..]
            .iter()
            .all(|p| matches!(p, Primitive::GlyphQuad { color, .. } if *color == Color::WHITE)));
        // not interactive: dispatch has no arm for a Badge
        let (mut scene, _l, _t, _a, id) = build_one(runtime, Badge::new("3"));
        assert!(!crate::dispatch_click(runtime, &mut scene, id));
    }

    // --- non-interactive kinds are inert to dispatch (SOUL §6.3) ---

    #[test]
    fn dispatch_click_basic_ignores_non_interactive_kinds() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        // ProgressBar / Divider are not routed here (they carry no actions), and the
        // hook returns false for any kind it does not own.
        let (mut scene, _l, _t, _a, id) = build_one(runtime, ProgressBar::new(1.0, 0.0, 2.0));
        assert!(!dispatch_click_basic(
            runtime,
            &mut scene,
            id,
            WidgetKind::ProgressBar
        ));
        let (mut scene, _l, _t, _a, id) = build_one(runtime, Divider::new());
        assert!(!dispatch_click_basic(
            runtime,
            &mut scene,
            id,
            WidgetKind::Divider
        ));
    }
}
