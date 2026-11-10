//! What a run is judged against: a fresh mount, an undecorated twin, and the
//! retained-state checks.

use std::panic::{catch_unwind, AssertUnwindSafe};

use schnellui::a11y::A11yTreeDump;
use schnellui::scene::{Primitive, WidgetId};
use schnellui::testing::fuzz::{invariants, strip_interactions};
use schnellui::App;

use crate::components::{Component, Mount};
use crate::ops::{aim_point, apply, quiesce, Op};
use crate::scrolling::{self, Watch};
use crate::{HeadlessHost, CLASSIC_THEMES, THEMES};

/// One primitive with what a person sees of it: kind, place, size and
/// colour. Atlas coordinates are left out, since they depend on the order in
/// which glyphs were first drawn, not on what is drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Ink {
    Fill([i32; 5], [u8; 4]),
    Stroke([i32; 6], [u8; 4]),
    Glyph([i32; 4], [u8; 4]),
    Image([i32; 4], [u8; 4]),
    Line([i32; 5], [u8; 4]),
}

fn tenth(v: f32) -> i32 {
    (v * 10.0).round() as i32
}

fn ink(p: &Primitive) -> Ink {
    let rgba = |c: schnellui::scene::Color| [c.r, c.g, c.b, c.a];
    match *p {
        Primitive::SolidRect {
            rect,
            color,
            corner_radius,
        } => Ink::Fill(
            [
                tenth(rect.x),
                tenth(rect.y),
                tenth(rect.width),
                tenth(rect.height),
                tenth(corner_radius),
            ],
            rgba(color),
        ),
        Primitive::StrokeRect {
            rect,
            color,
            corner_radius,
            width,
        } => Ink::Stroke(
            [
                tenth(rect.x),
                tenth(rect.y),
                tenth(rect.width),
                tenth(rect.height),
                tenth(corner_radius),
                tenth(width),
            ],
            rgba(color),
        ),
        Primitive::GlyphQuad { rect, color, .. } => Ink::Glyph(
            [
                tenth(rect.x),
                tenth(rect.y),
                tenth(rect.width),
                tenth(rect.height),
            ],
            rgba(color),
        ),
        Primitive::ImageQuad { rect, tint, .. } => Ink::Image(
            [
                tenth(rect.x),
                tenth(rect.y),
                tenth(rect.width),
                tenth(rect.height),
            ],
            rgba(tint),
        ),
        Primitive::Line {
            from,
            to,
            width,
            color,
        } => Ink::Line(
            [
                tenth(from.x),
                tenth(from.y),
                tenth(to.x),
                tenth(to.y),
                tenth(width),
            ],
            rgba(color),
        ),
    }
}

/// Everything a visible node paints, in tree order, with its kind and
/// accessible name to make a difference readable.
pub type Snapshot = Vec<(String, Vec<Ink>)>;

/// The visible tree's paint, node by node. Fully transparent primitives are
/// dropped: they paint nothing, and a hidden tooltip is one.
pub fn snapshot(app: &App) -> Snapshot {
    let scene = app.scene();
    let mut out = Vec::new();
    let mut stack: Vec<WidgetId> = scene.root().into_iter().collect();
    while let Some(id) = stack.pop() {
        if !scene.is_visible(id) {
            continue;
        }
        let Some(node) = scene.node(id) else { continue };
        stack.extend(node.children.iter().rev().copied());
        let inks: Vec<Ink> = scene
            .paint(id)
            .map(|p| {
                p.primitives
                    .iter()
                    .filter(|p| !transparent(p))
                    .map(ink)
                    .collect()
            })
            .unwrap_or_default();
        let name = scene
            .a11y(id)
            .and_then(|a| a.name.clone())
            .unwrap_or_default();
        out.push((format!("{:?} {name:?}", node.kind), inks));
    }
    out
}

fn transparent(p: &Primitive) -> bool {
    match *p {
        Primitive::SolidRect { color, .. }
        | Primitive::StrokeRect { color, .. }
        | Primitive::GlyphQuad { color, .. }
        | Primitive::Line { color, .. } => color.a == 0,
        Primitive::ImageQuad { tint, .. } => tint.a == 0,
    }
}

/// The accessibility tree without widget ids, the focus pointer and the
/// `focused` state.
pub fn semantics(app: &App) -> A11yTreeDump {
    schnellui::testing::structure(&schnellui::a11y::dump_tree(app.scene()))
}

/// The first node that paints differently, with its differing primitives.
pub fn diff(label_a: &str, a: &Snapshot, label_b: &str, b: &Snapshot) -> Option<String> {
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i), b.get(i));
        if x == y {
            continue;
        }
        let mut lines = String::new();
        if let (Some((kx, px)), Some((ky, py))) = (x, y) {
            if kx != ky {
                return Some(format!("node {i}: {label_a} has {kx}, {label_b} has {ky}"));
            }
            for j in 0..px.len().max(py.len()) {
                if px.get(j) != py.get(j) {
                    lines.push_str(&format!(
                        "  [{j}] {label_a}: {:?}\n      {label_b}: {:?}\n",
                        px.get(j),
                        py.get(j)
                    ));
                }
            }
            return Some(format!("node {i} {kx} paints differently\n{lines}"));
        }
        return Some(format!(
            "node {i}: {label_a} {:?} vs {label_b} {:?}",
            x.map(|n| &n.0),
            y.map(|n| &n.0)
        ));
    }
    None
}

pub fn semantics_diff(label_a: &str, a: &App, label_b: &str, b: &App) -> Option<String> {
    fn flat(node: &schnellui::a11y::A11yNodeDump, depth: usize, out: &mut Vec<String>) {
        out.push(format!(
            "{:indent$}{} {:?} value {:?} {:?}",
            "",
            node.role,
            node.name,
            node.value,
            node.state,
            indent = depth * 2
        ));
        for child in &node.children {
            flat(child, depth + 1, out);
        }
    }
    let lines = |app: &App| {
        let mut out = Vec::new();
        if let Some(root) = &semantics(app).root {
            flat(root, 0, &mut out);
        }
        out
    };
    let (x, y) = (lines(a), lines(b));
    let i = (0..x.len().max(y.len())).find(|i| x.get(*i) != y.get(*i))?;
    Some(format!(
        "accessibility trees differ at node {i}\n  {label_a}: {}\n  {label_b}: {}",
        x.get(i).map_or("(none)", String::as_str),
        y.get(i).map_or("(none)", String::as_str)
    ))
}

/// Runs `body`, turning a panic inside the app into an error that proptest
/// can shrink like any other failure.
pub fn guarded(body: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(result) => result,
        Err(panic) => Err(format!(
            "panic: {}",
            panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default()
        )),
    }
}

fn check(component: &Component, host: &mut HeadlessHost, when: &str) -> Result<(), String> {
    // Mid-burst no frame has laid out what the input changed, so geometry
    // still describes the last drawn frame. Check once the burst is drawn.
    if host.batching() {
        return Ok(());
    }
    invariants(host.app_mut()).map_err(|e| format!("{when}: {e}"))?;
    scrolling::settled(host).map_err(|e| format!("{when}: {e}"))?;
    (component.check)(host.app()).map_err(|e| format!("{when}: {e}"))
}

/// Applies `op` to `host`, then runs every check that holds after any op:
/// the frame's own checks, and the rules that compare it with the frame
/// before.
fn run_op(
    component: &Component,
    host: &mut HeadlessHost,
    op: &Op,
    mounted: (u32, u32),
    plain: bool,
    when: &str,
) -> Result<(), String> {
    let watch = Watch::before(host, op);
    apply(host, op, mounted, plain);
    check(component, host, when)?;
    watch.after(host, op).map_err(|e| format!("{when}: {e}"))
}

/// Same input to a decorated host and an undecorated twin. Their
/// accessibility trees match after every step, the retained-state checks
/// hold after every step, and once both are quiet they paint the same.
pub fn twin(component: &Component, mount: Mount, ops: &[Op]) -> Result<(), String> {
    guarded(|| {
        let mut decorated = (component.mount)(&mount);
        let mut plain = (component.mount)(&Mount {
            theme: strip_interactions(mount.theme),
            ..mount
        });
        let mut faded = false;
        for (step, op) in ops.iter().enumerate() {
            let when = format!("after step {step} {op:?}");
            apply(&mut plain, op, mount.size, true);
            run_op(component, &mut decorated, op, mount.size, false, &when)?;
            // The two fade between different themes, and interpolation can
            // round a frame of one onto the theme it already has, so one
            // host may skip a rebuild the other makes. A rebuild resets what
            // the app does not hold (an open context menu, a deferred
            // remount), so after a fade only the end state counts.
            faded |= matches!(op, Op::ThemeFade(..));
            if faded || decorated.batching() {
                continue;
            }
            if let Some(d) = semantics_diff("decorated", decorated.app(), "plain", plain.app()) {
                return Err(format!("{when}: {d}"));
            }
        }
        quiesce(&mut decorated);
        quiesce(&mut plain);
        check(component, &mut decorated, "after quiescing")?;
        match diff(
            "decorated",
            &snapshot(decorated.app()),
            "plain",
            &snapshot(plain.app()),
        ) {
            Some(d) => Err(format!("a decoration outlived its state: {d}")),
            None => Ok(()),
        }
    })
}

/// Drives a host through `ops`, then to the component's canonical state, and
/// compares it with a fresh mount at the final size and theme driven to the
/// same canonical state. Any input history must end where a fresh app ends.
pub fn history(component: &Component, mount: Mount, ops: &[Op]) -> Result<(), String> {
    let Some(canonical) = component.canonical else {
        return Ok(());
    };
    guarded(|| {
        let mut used = (component.mount)(&mount);
        let mut theme = mount.theme;
        for (step, op) in ops.iter().enumerate() {
            if let Op::Theme(i) | Op::ThemeFade(i, _) = op {
                theme = crate::THEMES[i % crate::THEMES.len()];
            }
            let when = format!("after step {step} {op:?}");
            run_op(component, &mut used, op, mount.size, false, &when)?;
        }
        quiesce(&mut used);
        canonical(&mut used);
        quiesce(&mut used);
        let size = used.app().size();
        let mut fresh = (component.mount)(&Mount {
            theme,
            size: (size.width as u32, size.height as u32),
            ..mount
        });
        if (size.width, size.height) != (fresh.app().size().width, fresh.app().size().height) {
            fresh.resize(size.width, size.height);
        }
        canonical(&mut fresh);
        quiesce(&mut fresh);
        if let Some(d) = semantics_diff("used", used.app(), "fresh", fresh.app()) {
            return Err(d);
        }
        match diff(
            "used",
            &snapshot(used.app()),
            "fresh",
            &snapshot(fresh.app()),
        ) {
            Some(d) => Err(format!("the used app paints unlike a fresh one: {d}")),
            None => Ok(()),
        }
    })
}

/// What the pointer and keyboard are doing while the window resizes.
#[derive(Clone, Debug)]
pub struct Grip {
    /// Ops replayed before the resize starts: hover, press, focus.
    pub before: Vec<Op>,
}

/// Resizes through `sizes`, one frame each, checking the retained state at
/// every frame. With a `grip`, its ops run first on both hosts, so hover,
/// press or focus may be live during the resize. At the end the window
/// must paint and read like a fresh mount at the final size (after the
/// component's canonical state, when it has one) and like its undecorated
/// twin.
pub fn resize(
    component: &Component,
    mount: Mount,
    grip: Option<&Grip>,
    sizes: &[(f32, f32)],
) -> Result<(), String> {
    guarded(|| {
        let mut host = (component.mount)(&mount);
        let mut plain = (component.mount)(&Mount {
            theme: strip_interactions(mount.theme),
            ..mount
        });
        let mut theme = mount.theme;
        if let Some(grip) = grip {
            for op in &grip.before {
                apply(&mut host, op, mount.size, false);
                apply(&mut plain, op, mount.size, true);
                if let Op::Theme(i) | Op::ThemeFade(i, _) = op {
                    theme = crate::THEMES[i % crate::THEMES.len()];
                }
            }
        }
        for (frame, (w, h)) in sizes.iter().enumerate() {
            host.resize(*w, *h);
            plain.resize(*w, *h);
            check(component, &mut host, &format!("frame {frame} at {w}x{h}"))?;
            if let Some(d) = semantics_diff("decorated", host.app(), "plain", plain.app()) {
                return Err(format!("frame {frame} at {w}x{h}: {d}"));
            }
        }
        quiesce(&mut host);
        quiesce(&mut plain);
        if let Some(d) = diff(
            "decorated",
            &snapshot(host.app()),
            "plain",
            &snapshot(plain.app()),
        ) {
            return Err(format!("a decoration outlived the resize: {d}"));
        }
        let (w, h) = *sizes.last().expect("at least one size");
        let mut fresh = (component.mount)(&Mount {
            theme,
            size: (w as u32, h as u32),
            ..mount
        });
        if grip.is_some() {
            let Some(canonical) = component.canonical else {
                return Ok(());
            };
            canonical(&mut host);
            quiesce(&mut host);
            canonical(&mut fresh);
            quiesce(&mut fresh);
        }
        if let Some(d) = semantics_diff("resized", host.app(), "fresh", fresh.app()) {
            return Err(d);
        }
        match diff(
            "resized",
            &snapshot(host.app()),
            "fresh",
            &snapshot(fresh.app()),
        ) {
            Some(d) => Err(format!(
                "the resized window paints unlike a fresh mount: {d}"
            )),
            None => Ok(()),
        }
    })
}

/// Same input to an animated host and a host under reduced motion.
/// Animation is presentation only: the accessibility trees match after
/// every step, the retained-state checks hold on every animation frame, and
/// once every animation has ended both paint the same.
pub fn animation(component: &Component, mount: Mount, ops: &[Op]) -> Result<(), String> {
    // Brutal and candy change shape and density, so a fade into them moves
    // every pointer target mid-flight and the two hosts stop seeing the same
    // clicks. Fade only between themes that share the classic shape.
    let classic = |i: usize| i % CLASSIC_THEMES;
    let mount = Mount {
        theme: if THEMES[..CLASSIC_THEMES].contains(&mount.theme) {
            mount.theme
        } else {
            THEMES[0]
        },
        ..mount
    };
    let ops: Vec<Op> = ops
        .iter()
        .map(|op| match *op {
            Op::Theme(i) => Op::Theme(classic(i)),
            Op::ThemeFade(i, ms) => Op::ThemeFade(classic(i), ms),
            ref other => other.clone(),
        })
        .collect();
    let ops = &ops[..];
    guarded(|| {
        let mut moving = (component.mount)(&Mount {
            animate: true,
            ..mount
        });
        let mut still = (component.mount)(&Mount {
            animate: false,
            ..mount
        });
        let mut themed = false;
        for (step, op) in ops.iter().enumerate() {
            // One pointer for both hosts. A host rebuilt under batching has
            // no layout to aim at until its next frame, while the screen
            // still shows the last one, so aim at whatever either host has
            // laid out and send both the same point.
            let op = match *op {
                Op::Aim { n, corner } => [&moving, &still]
                    .into_iter()
                    .find_map(|host| {
                        let p = aim_point(host.app(), n, corner)?;
                        let size = host.app().size();
                        Some(Op::Wander(p.x / size.width, p.y / size.height))
                    })
                    .unwrap_or_else(|| op.clone()),
                // The still host rebuilds at once, the animated one frame by
                // frame, so a press across a theme change would see the tree
                // rebuilt under it in one host only, and a rebuild drops
                // what a press under way was doing (a double-click's word
                // drag, a deferred remount). Keep presses and theme changes
                // apart here; `twin` covers presses during a fade, where
                // both hosts rebuild on the same frames.
                Op::Press(_) if moving.theme_fading() => continue,
                Op::Theme(_) | Op::ThemeFade(..) if still.held() => continue,
                ref other => other.clone(),
            };
            apply(&mut still, &op, mount.size, false);
            let when = format!("after step {step} {op:?}");
            run_op(component, &mut moving, &op, mount.size, false, &when)?;
            // A theme change rebuilds the still host at once and the
            // animated one frame by frame, and a rebuild resets what the
            // app does not hold (an open context menu, a press under way, a
            // remount deferred under a held button). After the first theme
            // change the two may differ until both are quiet, so from there
            // on only the per-frame checks and the end state count. Mid-burst
            // neither has drawn what the input did.
            themed |= matches!(op, Op::Theme(_) | Op::ThemeFade(..));
            if themed || still.batching() {
                continue;
            }
            if let Some(d) = semantics_diff("animated", moving.app(), "still", still.app()) {
                return Err(format!("{when}: {d}"));
            }
        }
        let mut fault = None;
        moving.set_batching(false);
        moving.run_animations(
            std::time::Duration::from_millis(16),
            std::time::Duration::from_secs(10),
            |host| {
                if fault.is_none() {
                    fault = invariants(host.app_mut()).err();
                }
            },
        );
        if let Some(f) = fault {
            return Err(format!("mid-animation: {f}"));
        }
        if moving.animating() {
            return Err("an animation is still running after ten seconds".into());
        }
        quiesce(&mut moving);
        quiesce(&mut still);
        if let Some(d) = semantics_diff("animated", moving.app(), "still", still.app()) {
            return Err(format!("after the animations ended: {d}"));
        }
        match diff(
            "animated",
            &snapshot(moving.app()),
            "still",
            &snapshot(still.app()),
        ) {
            Some(d) => Err(format!("the animations ended somewhere else: {d}")),
            None => Ok(()),
        }
    })
}

/// How fast input arrives.
#[derive(Clone, Copy, Debug)]
pub enum Pace {
    /// Every event within one display frame, drawn once at the end.
    Burst,
    /// A frame and 400 ms of virtual time after every event.
    Slow,
    /// Per event, from the seed: inside a burst, one frame later, or after
    /// a wait of up to a second.
    Random(u64),
}

fn paced(component: &Component, mount: Mount, ops: &[Op], pace: Pace) -> HeadlessHost {
    let mut host = (component.mount)(&mount);
    let mut seed = match pace {
        Pace::Random(seed) => seed | 1,
        _ => 0,
    };
    if matches!(pace, Pace::Burst) {
        host.set_batching(true);
    }
    for op in ops {
        apply(&mut host, op, mount.size, false);
        // Two presses close in time and place are a double click, so one input
        // read at two speeds is two different inputs. This property is about
        // where input lands, not how it is timed: every press is a single click.
        host.forget_clicks();
        match pace {
            Pace::Burst => {}
            Pace::Slow => host.advance(std::time::Duration::from_millis(400)),
            Pace::Random(_) => {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                match seed % 4 {
                    0 => host.set_batching(true),
                    1 => host.set_batching(false),
                    2 => host.advance(std::time::Duration::from_millis(16)),
                    _ => host.advance(std::time::Duration::from_millis(seed % 1000)),
                }
            }
        }
    }
    quiesce(&mut host);
    host
}

/// The same keyboard and pointer input at three paces: a single burst, one
/// event every 400 ms, and a random mix. Where the input lands cannot depend
/// on how fast it came, so all three end painting and reading the same.
pub fn pace(component: &Component, mount: Mount, ops: &[Op], seed: u64) -> Result<(), String> {
    guarded(|| {
        let slow = paced(component, mount, ops, Pace::Slow);
        for pace in [Pace::Burst, Pace::Random(seed)] {
            let other = paced(component, mount, ops, pace);
            let label = format!("{pace:?}");
            if let Some(d) = semantics_diff(&label, other.app(), "Slow", slow.app()) {
                return Err(d);
            }
            if let Some(d) = diff(
                &label,
                &snapshot(other.app()),
                "Slow",
                &snapshot(slow.app()),
            ) {
                return Err(format!("the pace of input changed the outcome: {d}"));
            }
        }
        Ok(())
    })
}
