//! Tracked app-level bindings: reactive paint colours and the reactive theme.
//!
//! Each binding owns a targeted [`Subscription`]: its closure runs once under
//! tracking, and afterwards only when a signal it read changes. A frame with no
//! relevant signal change drains an empty ready queue and calls no closure, so
//! the cost of a frame is proportional to the bindings that changed, not to the
//! number registered (SOUL §1, the same mechanism as the widgets' dynamic slots).

use schnellui::scene::{Color, Scene, WidgetId};
use schnellui::signal::{Subscription, SubscriptionGroup};
use slotmap::{Key, KeyData, SecondaryMap, SlotMap};

slotmap::new_key_type! {
    /// Identity of one paint binding; its FFI form is the subscription key.
    struct PaintKey;
}

struct PaintEntry {
    node: WidgetId,
    eval: Box<dyn FnMut() -> Color + 'static>,
    subscription: Subscription,
    /// Last colour the closure returned, re-applied without calling it when
    /// widget code repaints the node (text change, wrapped relayout, reposition).
    last: Option<Color>,
}

/// Reactive `|| -> Color` bindings, one per painted node.
pub(crate) struct PaintBindings {
    group: SubscriptionGroup,
    entries: SlotMap<PaintKey, PaintEntry>,
    /// Node -> binding, so `reapply` finds a repainted node's binding directly.
    by_node: SecondaryMap<WidgetId, PaintKey>,
    /// Keys that must run on the next frame regardless of signal readiness
    /// (fresh bindings, and bindings moved onto a remounted node).
    forced: Vec<u64>,
    /// Reused drain buffer.
    ready: Vec<u64>,
}

impl PaintBindings {
    pub(crate) fn new() -> Self {
        Self {
            group: SubscriptionGroup::new(),
            entries: SlotMap::with_key(),
            by_node: SecondaryMap::new(),
            forced: Vec::new(),
            ready: Vec::new(),
        }
    }

    /// Registers a binding. Its first evaluation happens on the next frame.
    pub(crate) fn push(&mut self, node: WidgetId, eval: Box<dyn FnMut() -> Color + 'static>) {
        let group = &self.group;
        let key = self.entries.insert_with_key(|key| PaintEntry {
            node,
            eval,
            subscription: group.subscribe(key.data().as_ffi()),
            last: None,
        });
        self.by_node.insert(node, key);
        self.forced.push(key.data().as_ffi());
    }

    /// Drops bindings whose node is gone. Survivors are not rescheduled: their
    /// nodes were not rebuilt, so they still carry the bound colour. Dropping
    /// an entry drops its `Subscription`, which unsubscribes it.
    pub(crate) fn retain_live(&mut self, scene: &Scene) {
        self.entries.retain(|_, e| scene.node(e.node).is_some());
        self.rebuild_index();
    }

    fn rebuild_index(&mut self) {
        self.by_node.clear();
        for (key, entry) in &self.entries {
            self.by_node.insert(entry.node, key);
        }
    }

    /// Re-targets every binding through `map`, dropping those it returns `None`
    /// for, and schedules the survivors for re-evaluation: the new node was
    /// built from scratch and does not yet carry the bound colour.
    pub(crate) fn remap(&mut self, mut map: impl FnMut(WidgetId) -> Option<WidgetId>) {
        let keys: Vec<PaintKey> = self.entries.keys().collect();
        for key in keys {
            let Some(entry) = self.entries.get_mut(key) else {
                continue;
            };
            match map(entry.node) {
                Some(node) => {
                    entry.node = node;
                    // The new node does not carry the colour yet.
                    entry.last = None;
                    self.forced.push(key.data().as_ffi());
                }
                None => {
                    if let Some(entry) = self.entries.remove(key) {
                        self.group.unsubscribe(entry.subscription);
                    }
                }
            }
        }
        self.rebuild_index();
    }

    pub(crate) fn node_ids(&self) -> impl Iterator<Item = WidgetId> + '_ {
        self.entries.values().map(|entry| entry.node)
    }

    /// Evaluates only the bindings that are ready (or forced) and writes their
    /// colour. `set_color` compares before writing, so an unchanged colour stays
    /// clean. Must run after the signal graph has been flushed.
    pub(crate) fn run(&mut self, scene: &mut Scene) {
        self.group.drain_ready_into(&mut self.ready);
        self.ready.append(&mut self.forced);
        for i in 0..self.ready.len() {
            let key = PaintKey::from(KeyData::from_ffi(self.ready[i]));
            let Some(entry) = self.entries.get_mut(key) else {
                continue;
            };
            if scene.node(entry.node).is_none() {
                if let Some(entry) = self.entries.remove(key) {
                    if self.by_node.get(entry.node) == Some(&key) {
                        self.by_node.remove(entry.node);
                    }
                    self.group.unsubscribe(entry.subscription);
                }
                continue;
            }
            // Re-tracking also re-arms the subscription for the next change.
            let color = entry.subscription.track(|| (entry.eval)());
            entry.last = Some(color);
            scene.set_color(entry.node, color);
        }
        self.ready.clear();
    }

    /// Re-applies the cached colour to every repainted node that has a binding.
    /// Widget code re-emits primitives (text change, wrapped layout, reposition)
    /// with theme colours; the binding must keep winning. The closure is not
    /// called, except for a binding that has never produced a colour. Cost
    /// follows the paint-dirty list, which is empty on a clean frame.
    pub(crate) fn reapply(&mut self, scene: &mut Scene) {
        if self.entries.is_empty() {
            return;
        }
        let mut i = 0;
        while i < scene.paint_dirty().len() {
            let node = scene.paint_dirty()[i];
            i += 1;
            let Some(&key) = self.by_node.get(node) else {
                continue;
            };
            let Some(entry) = self.entries.get_mut(key) else {
                continue;
            };
            let color = match entry.last {
                Some(color) => color,
                None => {
                    let color = entry.subscription.track(|| (entry.eval)());
                    entry.last = Some(color);
                    color
                }
            };
            scene.set_color(node, color);
        }
    }
}

/// The reactive theme closure with its own subscription.
pub(crate) struct ThemeBinding {
    group: SubscriptionGroup,
    subscription: Subscription,
    pub(crate) eval: Box<dyn FnMut() -> schnellui::widgets::Theme>,
    /// Evaluate on the next frame even if no signal changed (fresh binding).
    force: bool,
    ready: Vec<u64>,
}

impl ThemeBinding {
    pub(crate) fn new(eval: Box<dyn FnMut() -> schnellui::widgets::Theme>) -> Self {
        let group = SubscriptionGroup::new();
        let subscription = group.subscribe(0);
        Self {
            group,
            subscription,
            eval,
            force: true,
            ready: Vec::new(),
        }
    }

    /// Runs the closure under tracking when a signal it read changed (or on the
    /// first frame) and returns its theme; `None` on a clean frame.
    pub(crate) fn poll(&mut self) -> Option<schnellui::widgets::Theme> {
        self.group.drain_ready_into(&mut self.ready);
        let due = std::mem::take(&mut self.force) || !self.ready.is_empty();
        self.ready.clear();
        if !due {
            return None;
        }
        Some(self.subscription.track(|| (self.eval)()))
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::*;
    use schnellui::scene::Primitive;
    use schnellui::signal::create_signal;
    use schnellui::widgets::Text;

    fn seeded_node(app: &mut App, root: WidgetId) -> WidgetId {
        let node = app.scene_mut().insert(WidgetKind::Button, Some(root));
        app.scene_mut().replace_primitives(
            node,
            [Primitive::SolidRect {
                rect: scene::Rect::new(0.0, 0.0, 10.0, 10.0),
                color: Color::BLACK,
                corner_radius: 0.0,
            }],
        );
        node
    }

    #[test]
    fn clean_frame_calls_no_paint_binding_and_a_change_calls_only_its_own() {
        let mut app = App::new(100, 100);
        let root = app.scene_mut().insert(WidgetKind::Column, None);
        app.scene_mut().set_root(root);
        let calls = Rc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
        let signals = [create_signal(1u8), create_signal(2u8)];
        let mut nodes = Vec::new();
        for i in 0..2 {
            let node = seeded_node(&mut app, root);
            nodes.push(node);
            let (calls, sig) = (calls.clone(), signals[i]);
            app.bind_paint(node, move || {
                calls[i].fetch_add(1, Ordering::SeqCst);
                Color::rgb(sig.get(), 0, 0)
            });
        }
        app.frame();
        assert_eq!(calls[0].load(Ordering::SeqCst), 1);
        assert_eq!(calls[1].load(Ordering::SeqCst), 1);

        for _ in 0..3 {
            app.frame();
        }
        assert_eq!(
            calls[0].load(Ordering::SeqCst),
            1,
            "clean frame ran a binding"
        );
        assert_eq!(
            calls[1].load(Ordering::SeqCst),
            1,
            "clean frame ran a binding"
        );

        signals[1].set(9);
        app.frame();
        assert_eq!(calls[0].load(Ordering::SeqCst), 1);
        assert_eq!(calls[1].load(Ordering::SeqCst), 2);
        assert!(matches!(
            app.scene().paint(nodes[1]).unwrap().primitives[0],
            Primitive::SolidRect { color, .. } if color == Color::rgb(9, 0, 0)
        ));
    }

    #[test]
    fn replacing_an_unrelated_subtree_does_not_rerun_paint_bindings() {
        use schnellui::widgets::{Column, View as _};
        let branch_ref = scene::ComponentRef::new();
        let mut app = App::mount_with_size(
            Column::new().child(Text::new("a")).child(
                Column::new()
                    .child(Text::new("branch"))
                    .with_ref(branch_ref),
            ),
            100,
            100,
        );
        let root = app.scene().root().unwrap();
        let calls = Rc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
        for i in 0..2 {
            let node = seeded_node(&mut app, root);
            let calls = calls.clone();
            app.bind_paint(node, move || {
                calls[i].fetch_add(1, Ordering::SeqCst);
                Color::BLACK
            });
        }
        app.frame();
        app.replace_subtree(branch_ref, Text::new("other")).unwrap();
        app.frame();
        app.frame();
        assert_eq!(calls[0].load(Ordering::SeqCst), 1);
        assert_eq!(calls[1].load(Ordering::SeqCst), 1);
    }

    const RED: Color = Color::rgb(255, 0, 0);

    fn glyph_colors(app: &App, node: WidgetId) -> Vec<Color> {
        app.scene()
            .paint(node)
            .map(|p| {
                p.primitives
                    .iter()
                    .filter_map(|p| match p {
                        Primitive::GlyphQuad { color, .. } => Some(*color),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn bound_dynamic_text_keeps_its_colour_after_a_text_change() {
        let text = create_signal(1u32);
        let mut app =
            App::mount_with_size(Text::dynamic(move || format!("n{}", text.get())), 200, 100);
        let node = app.scene().root().unwrap();
        app.bind_paint(node, || RED);
        app.frame();
        assert!(glyph_colors(&app, node).iter().all(|c| *c == RED));
        text.set(22);
        for _ in 0..3 {
            app.frame();
        }
        let colors = glyph_colors(&app, node);
        assert!(!colors.is_empty());
        assert!(colors.iter().all(|c| *c == RED), "{colors:?}");
    }

    #[test]
    fn bound_wrapped_text_keeps_its_colour_after_resize() {
        let mut app = App::mount_with_size(
            Text::new("a fairly long line of text that wraps")
                .wrap(schnellui::widgets::WrapMode::Word),
            200,
            100,
        );
        let node = app.scene().root().unwrap();
        app.bind_paint(node, || RED);
        app.frame();
        app.resize(120.0, 100.0);
        app.frame();
        app.frame();
        let colors = glyph_colors(&app, node);
        assert!(!colors.is_empty());
        assert!(colors.iter().all(|c| *c == RED), "{colors:?}");
    }

    #[test]
    fn theme_binding_is_not_called_on_a_clean_frame() {
        let dark = create_signal(false);
        let calls = Rc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let mut app = App::mount_themed(theme::LIGHT, || Text::new("themed"));
        assert!(app.bind_theme(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            if dark.get() {
                theme::DARK
            } else {
                theme::LIGHT
            }
        }));
        app.frame();
        let after_first = calls.load(Ordering::SeqCst);
        assert_eq!(after_first, 1);
        for _ in 0..3 {
            app.frame();
        }
        assert_eq!(calls.load(Ordering::SeqCst), after_first);
        dark.set(true);
        app.frame();
        assert_eq!(calls.load(Ordering::SeqCst), after_first + 1);
        assert_eq!(app.active_theme(), theme::DARK);
    }
}
