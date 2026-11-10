//! Double and triple clicks: how fast one may come and still count, and when
//! two clicks are not one.
//!
//! The window host counts a press as the next click of a multi-click when it
//! lands on the same text field as the press before, no more than
//! [`MULTI_CLICK_INTERVAL`] after it and no more than [`MULTI_CLICK_SLOP`]
//! away on either axis. Two clicks select a word, three a line. Every other
//! control takes each click on its own.

use std::time::Duration;

use proptest::prelude::*;
use schnellui::scene::Point;
use schnellui::testing::fuzz::{MULTI_CLICK_INTERVAL, MULTI_CLICK_SLOP};

use crate::components::{Component, Mount};
use crate::ops::{aim_point, apply, quiesce, Op};
use crate::oracle::{diff, guarded, semantics_diff, snapshot};
use crate::{HeadlessHost, PointerButton};

/// The multi-click interval in milliseconds.
pub fn interval_ms() -> u32 {
    MULTI_CLICK_INTERVAL.as_millis() as u32
}

/// A double or triple click with the left button on one widget.
#[derive(Clone, Debug)]
pub struct MultiClick {
    /// The target, as `Op::Aim` names it.
    pub n: u8,
    pub corner: bool,
    /// Two or three.
    pub count: u8,
    /// How far each later press lands from the one before, in logical pixels.
    /// A hand does not hold still.
    pub drift: [(f32, f32); 2],
    /// Per click, how long the button stays down, in milliseconds.
    pub hold: [u32; 3],
    /// How long after a release the next press comes, in milliseconds.
    pub pause: [u32; 2],
}

/// A press-to-press gap inside the interval, split into hold and pause. The
/// ends of the range come up often: no time at all, and the last millisecond
/// that still counts.
fn quick_gap() -> impl Strategy<Value = (u32, u32)> {
    let total = prop_oneof![
        1 => Just(0u32),
        1 => Just(interval_ms()),
        4 => 0..=interval_ms(),
    ];
    (total, 0.0f32..=1.0).prop_map(|(total, share)| {
        let hold = ((total as f32 * share) as u32).min(total);
        (hold, total - hold)
    })
}

/// A drift that stays inside the slop on both axes.
fn near() -> impl Strategy<Value = (f32, f32)> {
    let reach = MULTI_CLICK_SLOP - 0.5;
    prop_oneof![
        2 => Just((0.0f32, 0.0f32)),
        3 => (-reach..=reach, -reach..=reach),
    ]
}

/// Any double or triple click a person can make on purpose.
pub fn any_multi_click() -> impl Strategy<Value = MultiClick> {
    (
        any::<u8>(),
        any::<bool>(),
        2u8..=3,
        [near(), near()],
        [quick_gap(), quick_gap()],
        0..=interval_ms(),
    )
        .prop_map(|(n, corner, count, drift, gaps, last_hold)| MultiClick {
            n,
            corner,
            count,
            drift,
            hold: [gaps[0].0, gaps[1].0, last_hold],
            pause: [gaps[0].1, gaps[1].1],
        })
}

/// What keeps a click from counting with the one before it.
#[derive(Clone, Copy, Debug)]
pub enum Break {
    /// It comes this many milliseconds after the press before, more than the
    /// interval.
    Late(u32),
    /// It lands this far from the press before, more than the slop on at
    /// least one axis.
    Far(f32, f32),
}

/// A run of two or three clicks of which click `at` (counting from zero)
/// breaks off from the one before.
#[derive(Clone, Debug)]
pub struct Broken {
    pub click: MultiClick,
    pub at: usize,
    pub how: Break,
}

pub fn any_broken() -> impl Strategy<Value = Broken> {
    let reach = MULTI_CLICK_SLOP + 0.5;
    let sign = || prop_oneof![Just(-1.0f32), Just(1.0f32)];
    let how = prop_oneof![
        1 => Just(Break::Late(interval_ms() + 1)),
        2 => (interval_ms() + 1..3000).prop_map(Break::Late),
        // Past the slop along x, along y, or both.
        1 => (reach..40.0f32, sign(), -40.0f32..40.0).prop_map(|(d, s, o)| Break::Far(d * s, o)),
        1 => (reach..40.0f32, sign(), -40.0f32..40.0).prop_map(|(d, s, o)| Break::Far(o, d * s)),
        1 => (sign(), sign()).prop_map(move |(sx, sy)| Break::Far(reach * sx, reach * sy)),
    ];
    (any_multi_click(), 1usize..3, how).prop_map(|(click, at, how)| Broken {
        at: at.min(click.count as usize - 1),
        click,
        how,
    })
}

/// Where `p` ends up when a host is told to point there: inside the window.
fn clamped(host: &HeadlessHost, p: Point) -> Point {
    let size = host.app().size();
    Point {
        x: p.x.clamp(0.0, (size.width - 1.0).max(0.0)),
        y: p.y.clamp(0.0, (size.height - 1.0).max(0.0)),
    }
}

fn outside_slop(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() > MULTI_CLICK_SLOP || (a.y - b.y).abs() > MULTI_CLICK_SLOP
}

/// Mounts the component and replays `before` on it, a frame after every op
/// and every press a single click, then lets go of the buttons.
fn prepared(component: &Component, mount: Mount, before: &[Op]) -> HeadlessHost {
    let mut host = (component.mount)(&mount);
    for op in before {
        apply(&mut host, op, mount.size, false);
        host.forget_clicks();
    }
    host.release(PointerButton::Left);
    host.release(PointerButton::Right);
    host.forget_clicks();
    host
}

fn ms(ms: u32) -> Duration {
    Duration::from_millis(u64::from(ms))
}

/// Both hosts read, select and paint the same, now and once quiet.
fn same(
    a: &mut HeadlessHost,
    label_a: &str,
    b: &mut HeadlessHost,
    label_b: &str,
) -> Result<(), String> {
    // Whatever waits on a resting pointer (a tooltip) has shown on both.
    a.advance(ms(2000));
    b.advance(ms(2000));
    for stage in ["after the clicks", "once quiet"] {
        if let Some(d) = semantics_diff(label_a, a.app(), label_b, b.app()) {
            return Err(format!("{stage}: {d}"));
        }
        let (sa, sb) = (a.app().selected_text(), b.app().selected_text());
        if sa != sb {
            return Err(format!(
                "{stage}: {label_a} selects {sa:?}, {label_b} selects {sb:?}"
            ));
        }
        if let Some(d) = diff(label_a, &snapshot(a.app()), label_b, &snapshot(b.app())) {
            return Err(format!("{stage}: {d}"));
        }
        quiesce(a);
        quiesce(b);
    }
    Ok(())
}

/// One double or triple click delivered inside a single display frame, and
/// the same click spread over frames with every press inside the interval of
/// the one before. Both are the same gesture, so both end the same: the same
/// word or line selected in a field, the same number of activations on
/// anything else.
pub fn multi_click(
    component: &Component,
    mount: Mount,
    before: &[Op],
    click: &MultiClick,
) -> Result<(), String> {
    guarded(|| {
        let mut fast = prepared(component, mount, before);
        let mut spread = prepared(component, mount, before);
        let Some(first) = aim_point(fast.app(), click.n, click.corner) else {
            return Ok(());
        };
        let count = click.count as usize;
        let mut points = vec![first];
        for (dx, dy) in &click.drift[..count - 1] {
            let last = points[points.len() - 1];
            points.push(clamped(
                &fast,
                Point {
                    x: last.x + dx,
                    y: last.y + dy,
                },
            ));
        }

        fast.set_batching(true);
        for p in &points {
            fast.pointer_move(*p);
            fast.press(PointerButton::Left);
            fast.release(PointerButton::Left);
        }
        fast.set_batching(false);

        let mut spent = 0;
        for (i, p) in points.iter().enumerate() {
            spread.pointer_move(*p);
            spread.press(PointerButton::Left);
            spread.advance(ms(click.hold[i]));
            spread.release(PointerButton::Left);
            spent += click.hold[i];
            if i + 1 < count {
                spread.advance(ms(click.pause[i]));
                spent += click.pause[i];
            }
        }
        fast.advance(ms(spent));
        same(&mut fast, "one frame", &mut spread, "spread over frames")
            .map_err(|e| format!("clicking at {points:?}: {e}"))
    })
}

/// A run of clicks in which one comes too late or lands too far from the one
/// before to count with it. That click starts over as a single click: the
/// run ends as it does on a host that was made to forget the earlier clicks
/// at that point.
pub fn not_a_multi_click(
    component: &Component,
    mount: Mount,
    before: &[Op],
    broken: &Broken,
) -> Result<(), String> {
    guarded(|| {
        let mut counted = prepared(component, mount, before);
        let mut single = prepared(component, mount, before);
        let click = &broken.click;
        let Some(first) = aim_point(counted.app(), click.n, click.corner) else {
            return Ok(());
        };
        let count = click.count as usize;
        let mut points = vec![first];
        for i in 1..count {
            let last = points[i - 1];
            let (dx, dy) = match broken.how {
                Break::Far(dx, dy) if i == broken.at => (dx, dy),
                _ => click.drift[i - 1],
            };
            let mut next = clamped(
                &counted,
                Point {
                    x: last.x + dx,
                    y: last.y + dy,
                },
            );
            if i == broken.at && matches!(broken.how, Break::Far(..)) && !outside_slop(last, next) {
                // The window edge cut the step short. Step the other way.
                next = clamped(
                    &counted,
                    Point {
                        x: last.x - dx,
                        y: last.y - dy,
                    },
                );
                if !outside_slop(last, next) {
                    return Ok(());
                }
            }
            points.push(next);
        }
        for (forget, host) in [(false, &mut counted), (true, &mut single)] {
            for (i, p) in points.iter().enumerate() {
                if forget && i == broken.at {
                    host.forget_clicks();
                }
                host.pointer_move(*p);
                host.press(PointerButton::Left);
                host.advance(ms(click.hold[i]));
                host.release(PointerButton::Left);
                if i + 1 < count {
                    // Time counts from press to press.
                    host.advance(ms(match broken.how {
                        Break::Late(gap) if i + 1 == broken.at => gap.saturating_sub(click.hold[i]),
                        _ => click.pause[i],
                    }));
                }
            }
        }
        same(&mut counted, "counted", &mut single, "forgetful")
            .map_err(|e| format!("clicking at {points:?}: {e}"))
    })
}
