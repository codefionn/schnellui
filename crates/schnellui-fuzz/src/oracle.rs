//! What a run is judged against: a fresh mount, an undecorated twin, and the
//! retained-state checks.

use std::panic::{catch_unwind, AssertUnwindSafe};

use schnellui::a11y::A11yTreeDump;
use schnellui::scene::{Primitive, WidgetId};
use schnellui::testing::fuzz::{invariants, strip_interactions};
use schnellui::App;

use crate::components::{Component, Mount};
use crate::ops::{apply, quiesce, Op};
use crate::HeadlessHost;

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

fn semantics_diff(label_a: &str, a: &App, label_b: &str, b: &App) -> Option<String> {
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
    invariants(host.app_mut()).map_err(|e| format!("{when}: {e}"))?;
    (component.check)(host.app()).map_err(|e| format!("{when}: {e}"))
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
        for (step, op) in ops.iter().enumerate() {
            apply(&mut decorated, op, mount.size, false);
            apply(&mut plain, op, mount.size, true);
            let when = format!("after step {step} {op:?}");
            check(component, &mut decorated, &when)?;
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
            apply(&mut used, op, mount.size, false);
            if let Op::Theme(i) = op {
                theme = crate::THEMES[i % crate::THEMES.len()];
            }
            check(component, &mut used, &format!("after step {step} {op:?}"))?;
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
                if let Op::Theme(i) = op {
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
