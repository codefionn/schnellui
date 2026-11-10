//! What must hold of scrolling after any input.
//!
//! [`settled`] judges a frame by itself: offsets inside their range, the
//! accessibility tree agreeing with the scene, and the hit test agreeing with
//! what is drawn. [`Watch`] remembers what an op started from, for the rules
//! that compare before and after: a wheel leaves hover on the control now under
//! a resting pointer, keyboard focus and the caret scroll into view, and a
//! theme switch keeps the offsets.

use schnellui::a11y::to_access_id;
use schnellui::scene::{Point, Rect, WidgetId, WidgetKind};
use schnellui::testing::fuzz::{hover_faults, reveal_target};
use schnellui::widgets::{is_in_subtree, scroll_metrics};
use schnellui::App;

use crate::ops::{actionable, Op};
use crate::{ClipboardAction, HeadlessHost, HostKey};

/// Slack for float rounding in rects and offsets.
const EPS: f32 = 0.5;

/// Every scroll viewport in the scene, in tree order.
pub fn viewports(app: &App) -> Vec<WidgetId> {
    let scene = app.scene();
    scene
        .preorder()
        .filter(|id| {
            scene
                .node(*id)
                .is_some_and(|n| n.kind == WidgetKind::Scroll)
        })
        .collect()
}

/// Every viewport's offset stays in `[0, max_offset]`, whatever resized,
/// grew or shrank under it.
pub fn offsets_in_range(app: &App) -> Result<(), String> {
    let scene = app.scene();
    for id in viewports(app) {
        let Some(m) = scroll_metrics(scene, id) else {
            continue;
        };
        let at = scene.scroll_offset(id);
        if at.x != 0.0 || at.y < 0.0 || at.y > m.max_offset + 0.01 || !at.y.is_finite() {
            return Err(format!(
                "scroll {id:?} sits at {at:?}, outside 0..={} (content {}, viewport {})",
                m.max_offset, m.content_height, m.viewport.height
            ));
        }
    }
    Ok(())
}

/// What assistive technology is told about each viewport is its scene offset:
/// the accessible value is the rounded offset, and the node's transform
/// shifts its content up by it.
pub fn a11y_matches_offsets(app: &App) -> Result<(), String> {
    let scene = app.scene();
    let ids = viewports(app);
    if ids.is_empty() {
        return Ok(());
    }
    for id in &ids {
        let at = scene.scroll_offset(*id).y;
        let value = scene.a11y(*id).and_then(|a| a.value.clone());
        let told: i64 = match &value {
            Some(v) => v
                .trim()
                .parse()
                .map_err(|_| format!("scroll {id:?} reads {v:?}"))?,
            None => 0,
        };
        if told != at.round() as i64 {
            return Err(format!(
                "scroll {id:?} reads {told} to assistive technology but sits at {at}"
            ));
        }
    }
    let update = schnellui::a11y::build_full_tree_update(scene);
    for id in &ids {
        let at = scene.scroll_offset(*id).y;
        let node = update
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == to_access_id(*id))
            .map(|(_, node)| node)
            .ok_or_else(|| format!("scroll {id:?} is missing from the accessibility tree"))?;
        // The root also carries the window scale in its transform.
        let [scale, _, _, _, _, shift] = node
            .transform()
            .map_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0], |t| t.as_coeffs());
        let expected = -f64::from(at) * scale;
        if (shift - expected).abs() > 0.01 {
            return Err(format!(
                "scroll {id:?} transforms its content by {shift} but sits at {at} (scale {scale})"
            ));
        }
    }
    Ok(())
}

fn related(app: &App, a: WidgetId, b: WidgetId) -> bool {
    let scene = app.scene();
    is_in_subtree(scene, a, b) || is_in_subtree(scene, b, a)
}

/// The control the pointer hits is the one drawn there. Each actionable
/// control's visible centre hits it, or something inside it or around it, and
/// whatever any other point hits does cover that point, as the renderer
/// places it (layout shifted by every viewport's offset, cut by its clip).
pub fn hits_follow_paint(app: &App) -> Result<(), String> {
    if viewports(app).is_empty() {
        return Ok(());
    }
    let scene = app.scene();
    // Floating layers (a context menu, a dialog, a popup) cover what is under
    // them: the pointer then hits the layer.
    let layers: Vec<WidgetId> = scene
        .preorder()
        .filter(|id| scene.is_overlay(*id) && scene.is_visible(*id))
        .collect();
    for id in actionable(app) {
        // A viewport is a container: the pointer hits what is inside it.
        // A link in rich text is laid out as inline fragments, one per line it
        // wraps over: its box is their bounding box, not where it is hit.
        let inline = scene
            .node(id)
            .and_then(|n| n.parent)
            .and_then(|p| scene.node(p))
            .is_some_and(|p| p.kind == WidgetKind::RichText);
        if inline || scene.node(id).is_some_and(|n| n.kind == WidgetKind::Scroll) {
            continue;
        }
        let drawn = scene.composited_rect(id);
        if drawn.width < 1.0 || drawn.height < 1.0 {
            continue;
        }
        let p = Point {
            x: drawn.x + drawn.width / 2.0,
            y: drawn.y + drawn.height / 2.0,
        };
        let covered = layers.iter().any(|l| {
            let r = scene.composited_rect(*l);
            r.width > 0.0 && r.height > 0.0 && r.contains(p) && !related(app, *l, id)
        });
        if covered {
            continue;
        }
        match app.hit_test(p) {
            Some(hit) if related(app, hit, id) => {}
            other => {
                return Err(format!(
                "{:?} {:?} is drawn at {drawn:?} but the pointer at {p:?} hits {other:?} ({:?})",
                scene.node(id).map(|n| n.kind),
                scene.a11y(id).and_then(|a| a.name.clone()),
                other.and_then(|h| scene.node(h)).map(|n| n.kind),
            ))
            }
        }
    }
    let size = app.size();
    for i in 0..9 {
        for j in 0..9 {
            let p = Point {
                x: size.width * (i as f32 + 0.5) / 9.0,
                y: size.height * (j as f32 + 0.5) / 9.0,
            };
            let Some(hit) = app.hit_test(p) else { continue };
            let drawn = scene.composited_rect(hit);
            let grown = Rect::new(
                drawn.x - EPS,
                drawn.y - EPS,
                drawn.width + 2.0 * EPS,
                drawn.height + 2.0 * EPS,
            );
            if !grown.contains(p) {
                return Err(format!(
                    "the pointer at {p:?} hits {:?} {hit:?}, which is drawn at {drawn:?}",
                    scene.node(hit).map(|n| n.kind),
                ));
            }
        }
    }
    Ok(())
}

/// What is wrong with hover under the resting pointer, `None` when it is on
/// the control the pointer is over or no button is up and the pointer is in.
fn hover_fault(host: &HeadlessHost) -> Option<String> {
    if host.held() {
        return None;
    }
    hover_faults(host.app(), host.pointer()?)
}

/// The names of the controls shown in viewport `id`: those with a visible
/// part in it.
fn shown_in(app: &App, id: WidgetId) -> Vec<String> {
    let scene = app.scene();
    let mut out = Vec::new();
    let mut stack: Vec<WidgetId> = scene
        .node(id)
        .map(|n| n.children.iter().rev().copied().collect())
        .unwrap_or_default();
    while let Some(c) = stack.pop() {
        let Some(node) = scene.node(c) else { continue };
        stack.extend(node.children.iter().rev().copied());
        let shown = scene.composited_rect(c);
        if shown.width < 1.0 || shown.height < 1.0 {
            continue;
        }
        if let Some(a) = scene.a11y(c).filter(|a| a.actions != 0) {
            out.extend(a.name.clone());
        }
    }
    out
}

fn focused_text(app: &App) -> Option<String> {
    let id = app.focused_widget()?;
    app.scene().a11y(id)?.value.clone()
}

/// Every check that judges a settled frame on its own.
pub fn settled(host: &HeadlessHost) -> Result<(), String> {
    let app = host.app();
    offsets_in_range(app)?;
    a11y_matches_offsets(app)?;
    hits_follow_paint(app)
}

/// What an op started from.
pub struct Watch {
    /// For a theme switch: what each viewport shows.
    shown: Vec<Vec<String>>,
    /// Whether a context menu was open: it takes the keys.
    menu: bool,
    /// For a clipboard command: the focused field's text.
    text: Option<String>,
    /// Whether hover was on the control under the pointer before the op.
    hover_right: bool,
    focus: Option<WidgetId>,
    offsets: Vec<f32>,
    batching: bool,
}

impl Watch {
    pub fn before(host: &HeadlessHost, op: &Op) -> Watch {
        let app = host.app();
        let shown = match op {
            Op::Theme(_) => viewports(app)
                .into_iter()
                .map(|id| shown_in(app, id))
                .collect(),
            _ => Vec::new(),
        };
        Watch {
            shown,
            menu: app.context_menu_is_open(),
            text: focused_text(app),
            hover_right: hover_fault(host).is_none(),
            focus: app.focused_widget(),
            offsets: viewports(app)
                .into_iter()
                .map(|id| app.scene().scroll_offset(id).y)
                .collect(),
            batching: host.batching(),
        }
    }

    /// The rules that compare the frame after `op` with the one before it.
    pub fn after(&self, host: &mut HeadlessHost, op: &Op) -> Result<(), String> {
        if host.batching() || self.batching {
            return Ok(());
        }
        match op {
            // Content that scrolled under a pointer at rest puts a different
            // control under it, and hover goes with the pointer. Only a
            // wheel that started from right hover is judged: hover left
            // behind by something else (a resize) is not the wheel's fault.
            Op::Wheel(_) | Op::WheelBurst(..) | Op::WheelAt(..) => {
                match hover_fault(host).filter(|_| self.hover_right) {
                    Some(f) => Err(format!("a wheel left hover behind: {f}")),
                    None => Ok(()),
                }
            }
            Op::Theme(_) => self.theme_keeps_offsets(host.app()),
            Op::Key(..) | Op::Chord(..) if self.menu => Ok(()),
            Op::Key(key, _) | Op::Chord(key) => {
                let moved = host.app().focused_widget() != self.focus;
                let edit = matches!(
                    key,
                    HostKey::Text(_)
                        | HostKey::Backspace
                        | HostKey::Delete
                        | HostKey::Left
                        | HostKey::Right
                        | HostKey::Home
                        | HostKey::End
                );
                self.focus_in_view(host, moved, edit)
            }
            // Copy changes neither the text nor the caret.
            // And only a cut or paste that changed the text edits it.
            Op::Clipboard(ClipboardAction::Cut | ClipboardAction::Paste) => {
                if self.text == focused_text(host.app()) {
                    return Ok(());
                }
                self.focus_in_view(host, false, true)
            }
            _ => Ok(()),
        }
    }

    /// A theme switch rebuilds the tree, and the offsets come through it, cut
    /// to what the rebuilt content allows. A viewport that keeps its place by
    /// a row instead of by pixels (a virtual list anchors on a key, and a theme
    /// of another density gives the rows other heights) passes by showing the
    /// same control at its top.
    fn theme_keeps_offsets(&self, app: &App) -> Result<(), String> {
        let ids = viewports(app);
        if ids.len() != self.offsets.len() {
            return Ok(());
        }
        for (i, (id, before)) in ids.into_iter().zip(&self.offsets).enumerate() {
            let Some(m) = scroll_metrics(app.scene(), id) else {
                continue;
            };
            let now = app.scene().scroll_offset(id).y;
            let kept = before.min(m.max_offset);
            let after = || shown_in(app, id);
            let overlaps = || self.shown[i].iter().any(|name| after().contains(name));
            if (now - kept).abs() > EPS && !overlaps() {
                return Err(format!(
                    "a theme switch moved scroll {id:?} from {before} to {now} (it can reach {})",
                    m.max_offset
                ));
            }
        }
        Ok(())
    }

    /// After a key moved focus, or a key edited a text field, the focused
    /// control, or the caret row of a text area, is on screen in every viewport
    /// around it.
    fn focus_in_view(
        &self,
        host: &mut HeadlessHost,
        moved: bool,
        edit: bool,
    ) -> Result<(), String> {
        let Some(id) = host.app().focused_widget() else {
            return Ok(());
        };
        let kind = host.app().scene().node(id).map(|n| n.kind);
        let text = matches!(kind, Some(WidgetKind::TextInput | WidgetKind::TextArea));
        if !(moved || edit && text) {
            return Ok(());
        }
        let Some(target) = reveal_target(host.app_mut(), id) else {
            return Ok(());
        };
        let scene = host.app().scene();
        let (offset, clip) = scene.composite_context(id);
        let shown = Rect::new(
            target.x + offset.x,
            target.y + offset.y,
            target.width,
            target.height,
        );
        let fits = shown.height <= clip.height + EPS;
        let seen = if fits {
            shown.y >= clip.y - EPS && shown.bottom() <= clip.bottom() + EPS
        } else {
            shown.y < clip.bottom() && shown.bottom() > clip.y
        };
        if seen {
            return Ok(());
        }
        // The target carries the focus ring. A control at the very end of its
        // content has its ring cut there whatever the viewport does, so with a
        // viewport resting at an end the control's own box is what must show.
        let mut pinned = false;
        let mut cur = schnellui::widgets::enclosing_scroll(scene, id);
        while let Some(viewport) = cur {
            if let Some(m) = scroll_metrics(scene, viewport) {
                pinned |= m.offset <= EPS || m.offset >= m.max_offset - EPS;
            }
            cur = schnellui::widgets::enclosing_scroll(scene, viewport);
        }
        if let (true, Some(rect)) = (pinned, scene.layout(id).map(|l| l.rect)) {
            let boxed = Rect::new(
                rect.x + offset.x,
                rect.y + offset.y,
                rect.width,
                rect.height,
            );
            if boxed.height > clip.height + EPS
                || (boxed.y >= clip.y - EPS && boxed.bottom() <= clip.bottom() + EPS)
            {
                return Ok(());
            }
        }
        Err(format!(
            "focused {kind:?} {id:?} is at {shown:?} but its viewports show {clip:?} ({})",
            if moved {
                "a key moved focus"
            } else {
                "a key edited it"
            }
        ))
    }
}
